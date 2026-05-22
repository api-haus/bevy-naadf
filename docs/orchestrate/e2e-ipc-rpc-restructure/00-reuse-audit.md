# Reuse audit — e2e harness restructure to IPC-RPC-controlled production app

> Re-implementation auditor pass. Scope: catalogue the *current* e2e harness
> shape and every building block an IPC-RPC restructure would reuse, extend,
> or be blocked by. **No design** — reuse verdicts only. All `file:line`
> verified against current HEAD (`ee87400`, branch `feat/android-build`).
>
> Goal under audit: make `bin/bevy-naadf` (production app) the
> system-under-test, driven externally over RPC-over-IPC by a test runner
> that spawns it in another process; retire e2e-as-in-app-driver-modes.

---

## Candidate table

| candidate | location (file:line) | what it does | reuse / extend / not applicable | one-line justification |
|---|---|---|---|---|
| `e2e/checks.rs` `PipelineScanResult` cross-world `Arc<Mutex<…>>` channel | `crates/bevy_naadf/src/e2e/checks.rs:37-44`, inserted into both worlds at `e2e/mod.rs:230,246,311` | A `Resource` wrapping `Arc<Mutex<Option<Result<(),String>>>>` cloned into the main world AND the `RenderApp`; render-world system writes, main-world reads. The project's only established cross-boundary data channel. | **extend** | The exact precedent an RPC layer needs for "render-world produces, external boundary consumes" — an RPC plugin would clone a similar handle to ferry query results out of systems. Not a transport, but the in-process half is here. |
| `e2e/readback.rs` `E2eScreenshot` + `shoot_primary_window` + `ScreenshotCaptured` observer | `crates/bevy_naadf/src/e2e/readback.rs:14-37`; `Framebuffer` wrapper at `framebuffer.rs:137-518` | Async `Screenshot::primary_window()` capture stashed into a `Resource`; `Framebuffer` decodes the `Image`, exposes `save_png`, region means, SSIM-feed pixels, degenerate/luminance checks. | **reuse** | Framebuffer capture + PNG encoding is the "output capture surface" open question #3 from `04-followup`; this entire module is transport-agnostic and an RPC `capture_framebuffer` verb wraps it as-is. |
| `bootstrap::BootstrapInputs` carrier + `build_app_with_bootstrap_inputs` fan-out | `crates/bevy_naadf/src/bootstrap.rs:48-225` | Transient boot-time config struct (8 typed per-domain fields, incl. `gate_mode: E2eGateMode`); `build_app_with_bootstrap_inputs` fans each field into a main-world `Resource` via overwrite-in-place `insert_resource`. | **extend** | The natural place to add an `rpc_enabled`/transport-config field; the fan-out is already the single funnel where an RPC `Plugin` would be conditionally added. The follow-up's "enum resource is the natural RPC-controllable handle" maps here. |
| `e2e/gate.rs` `E2eGateMode` enum resource | `crates/bevy_naadf/src/e2e/gate.rs:48-88` | 11-variant `#[derive(Resource)]` enum; driver state machine + 6 `pin_*_camera` systems + `window_for_gate_mode` branch on it. Collapsed the 10 e2e-mode booleans (Step 6). | **extend** | An RPC `set_resource(E2eGateMode::…)` verb flips this directly (per `04-followup` § "Relationship to the in-flight AppArgs refactor"). The enum stays load-bearing; what an RPC layer removes is the *argv parser* that sets it, not the enum. |
| `e2e/driver.rs` `e2e_driver` state machine + `E2eState`/`E2eOutcome` | `crates/bevy_naadf/src/e2e/driver.rs:61` (`E2ePhase`, 26 variants), `:254` (`E2eState`), `:263` (`E2eOutcome.gate_result`), `:452+` (`e2e_driver`, ~1400 lines) | A giant `match state.phase` that advances a fixed frame budget, fires brushes/camera-pins, captures framebuffers, runs assertions, writes `AppExit`. ONE entry per gate; 22 entry-flag routing. | **not applicable (as-is) — partial extraction** | This IS the "in-app driver modes" the restructure exists to *delete*. Its phase steps (warmup → capture → edit → capture → assert) are the per-gate scenario logic that must move into RPC test bodies. The frame-stepping/capture/edit *primitives* it calls are reusable; the `match`-over-phase orchestration is the thing being retired. |
| `bin/e2e_render.rs` 3-layer argv parser | `crates/bevy_naadf/src/bin/e2e_render.rs:1-547` (whole binary) | Hand-rolled (no `clap`) Layer-1 short-circuits / Layer-2 boot commands / Layer-3 post-app validations; 22 modes; subprocess re-exec for compare gates. | **not applicable** | The restructure's explicit target for deletion (per `04-followup` § "Why this is structurally cleaner": "`bin/e2e_render` deletes"). The subprocess-spawn-and-orchestrate pattern at `:172-249` is the only transferable idea — a test runner spawns the SUT the same way. |
| `editor/` brushes + `EditorState` resource + `EditorPlugin` | `crates/bevy_naadf/src/editor/mod.rs:44-225`; brushes `tools.rs:226-277` (`paint_brush`/`cube_brush`/`sphere_brush`) | Runtime voxel-edit surface: `EditorState` resource (tool/radius/erase/pos), `apply_edit_tool` Update system casts pick-rays, dispatches brushes. Gated `add_hud`-off in e2e config. | **extend** | The closest existing "in-process functional-control surface an RPC layer could wrap" (open question #5). `EditorState` is a mutable resource and the brush fns take `(&mut WorldData, pos, radius, ty)` — directly callable as RPC `apply_brush` primitives. Currently driven by mouse input, not programmatically. |
| `e2e/ssim.rs` `ssim_compare_command` + `parse_ssim_compare_args` | `crates/bevy_naadf/src/e2e/ssim.rs` (invoked `bin/e2e_render.rs:211-222`) | Pure CPU PNG-diff: load two PNGs, compute SSIM, exit per `[min,max)` band. No App, no GPU, no window. | **reuse** | Transport-agnostic pure function; an RPC test body calls it on two captured framebuffers in-process. Survives the restructure unchanged as a library function (or a thin developer CLI). |

---

## E2e-entry enumeration (verified at HEAD)

`bin/e2e_render.rs` dispatches 22 entries across 3 parser layers. The prior
doc `03-e2e-as-tests-investigation.md` § 1.2 enumerated these; I verified the
routing in `parse_top_level_short_circuit` (`:171-188`), `parse_gate_command`
(`:259-330`), and `parse_post_app_validations` (`:470-477`). The table holds —
**line-number drift noted in Borderline calls below.**

**Layer 2 — booted-window gates (open a real winit window via `DefaultPlugins`):**

| Flag | `run_*` entry | Window | Headless? | Captures framebuffer? | Needs GPU |
|---|---|---|---|---|---|
| (none / default) `Standard` | `e2e::run_e2e_render` (`e2e/mod.rs:398`) | 256×256 | no — real window | yes (`e2e_latest.png`) | yes |
| `--vox-e2e` | `vox_e2e::run_vox_e2e` (`vox_e2e.rs:346`) | 256×256 | no | yes | yes |
| `--oasis-edit-visual` | `oasis_edit_visual::run_oasis_edit_visual` (`oasis_edit_visual.rs:182`) | 256×256 | no | yes (before/after) | yes |
| `--small-edit-visual` | `small_edit_visual::run_small_edit_visual` (`small_edit_visual.rs:209`) | 256×256 | no | yes (before/after) | yes |
| `--small-edit-repro` | `small_edit_repro::run_small_edit_repro` (`small_edit_repro.rs:124`) | 1920×1080 | no | yes (before/after) | yes |
| `--vox-gpu-construction` | `vox_gpu_construction::run_vox_gpu_construction` (`vox_gpu_construction.rs:193`) | 256×256 | no | yes (before/after) | yes |
| `--vox-gpu-oracle-cpu` | `vox_gpu_oracle::run_vox_gpu_oracle_cpu_phase` (`vox_gpu_oracle.rs:257`) | 256×256 | no | yes (`oracle_cpu.png`) | yes |
| `--vox-gpu-oracle-gpu` | `vox_gpu_oracle::run_vox_gpu_oracle_gpu_phase` (`vox_gpu_oracle.rs:310`) | 256×256 | no | yes (`oracle_gpu.png`) | yes |
| `--vox-web-parity-skybox` | `vox_web_parity::run_vox_web_parity_skybox_phase` (`vox_web_parity.rs:151`) | 256×256 | no | yes | yes |
| `--vox-web-parity-loaded` | `vox_web_parity::run_vox_web_parity_loaded_phase` (`vox_web_parity.rs:181`) | 256×256 | no | yes | yes |
| `--vox-horizon-native` | `vox_horizon_parity::run_vox_horizon_native_phase` (`vox_horizon_parity.rs:131`) | 1280×720 | no | yes | yes |
| `--resize-test` | `bin/e2e_render.rs::run_resize_test` (`:370`) | 800×600→1920×1080→2000×1000 | no — **Hyprland-only**; bails without `HYPRLAND_INSTANCE_SIGNATURE` | yes (3 PNGs) | yes |
| `--entities` | `bin/e2e_render.rs::EntitiesBoot` arm (`:345-362`) | 256×256 | no | yes | yes |

**Layer 1 — no-Bevy-boot short-circuits:**

| Flag | Entry | Headless? | GPU |
|---|---|---|---|
| `--vox-gpu-oracle` | `vox_gpu_oracle::run_vox_gpu_oracle_compare` (`vox_gpu_oracle.rs:361`) — spawns cpu+gpu subprocesses, SSIM-compares | yes (no App; subprocesses do) | no (itself) |
| `--vox-web-parity` | `vox_web_parity::run_vox_web_parity_compare` (`vox_web_parity.rs:228`) — spawns skybox+loaded subprocesses, SSIM-compares | yes (no App) | no (itself) |
| `--ssim-compare <a> <b>` | `ssim::ssim_compare_command` (`e2e/ssim.rs`) — pure PNG diff | yes | no |
| `--validate-gpu-construction-scaled` | `render::construction::validate_gpu_construction_scaled` (`bin/e2e_render.rs:229`) — `MinimalPlugins`+`RenderPlugin`, no window | **yes — already headless** | yes (no surface) |
| `--validate-gpu-construction-production` | `render::construction::validate_gpu_construction_production_scale` (`bin/e2e_render.rs:240`) | **yes — already headless** | yes (no surface) |

**Layer 3 — post-app validation tails (`MinimalPlugins`+`RenderPlugin`, no window; compose orthogonally with a boot command):**

| Flag | Entry | Headless? |
|---|---|---|
| `--validate-gpu-construction` | `render::construction::validate_gpu_construction` (`bin/e2e_render.rs:485`) | **yes — already headless** |
| `--entities` (post-app) | `render::construction::validate_entity_handler` (`:505`) | **yes — already headless** |
| `--edit-mode` | `render::construction::validate_edit_mode` (`:517`) | **yes — already headless** |
| `--runtime-edit-mode` | `render::construction::validate_runtime_edit_mode` (`:534`) | **yes — already headless** |

**Summary:** 13 booted-window gates · 5 Layer-1 short-circuits · 4 Layer-3
validators. The 7 already-headless validators (the 5 Layer-1/3 `validate_*`
fns + `ssim_compare_command` + `--validate-gpu-construction`) are *not*
in-app driver modes — they are pure `pub fn … -> Result`/`u8` calls that
build+tear-down their own `MinimalPlugins` App. An IPC-RPC restructure
**leaves them entirely outside its scope** — they should become `tests/`
or stay library calls regardless (this is `03`'s separable "Option C").

---

## Production app boot path (the would-be SUT)

- `bin/bevy-naadf` → `main.rs:34` `fn main() -> AppExit`. Argv: only `--vox <path>`
  (`main.rs:48-57`) → resolves into `GridPreset::Vox`. **No e2e flags, no RPC
  surface.** Native path calls `build_app_with_budget(AppConfig::windowed(), grid_preset)`
  (`lib.rs:161`) then `.run()`. wasm32 path does an async adapter probe then
  `build_app_with_bootstrap_inputs` (`main.rs:95-123`).
- `android_main.rs` → JNI entry; also routes through `build_app_with_budget` on
  a default `BootstrapInputs`. No CLI on Android.
- **Shared core:** `build_app_core` (`lib.rs:191`) is the single plugin-pyramid
  funnel. `build_app` (`:136`) = production direct; `build_app_with_bootstrap_inputs`
  (`bootstrap.rs:148`) = the fan-out wrapper every e2e gate uses.
- **Divergence between `bevy-naadf` and `e2e_render`:** only `AppConfig`
  (`AppConfig::windowed()` vs `AppConfig::e2e()`) — the latter flips
  `add_e2e_systems` (installs `e2e_driver` + `WinitSettings{Continuous,Continuous}`),
  `add_hud=false`, `synchronous_pipeline_compilation=true`, fixed window size.
  Both use the real `DefaultPlugins` + `WinitPlugin`. **The production app
  already IS structurally the same App** — making it the SUT means adding an
  RPC server plugin and *not* adding `add_e2e_systems`.

---

## IPC / RPC / process-control machinery — inventory

**There is no IPC, RPC, socket, pipe, or named-channel code anywhere in the
workspace.** Verified by `git grep` for `UnixListener`, `TcpListener`,
`named_pipe`, `interprocess`, `bevy_remote`, `RemotePlugin`, `BrpPlugin`,
`"remote"`, `brp` across `crates/` — zero matches in source.

- **Bevy Remote Protocol (BRP):** the `bevy` dependency
  (`Cargo.toml:48-50`) enables only `["free_camera"]` + `asset_processor`
  (native) + `android-game-activity` (Android). **`bevy_remote` / the `remote`
  feature is NOT enabled.** Open question #1 of `04-followup` ("does BRP cover
  the surface") is unanswered by the codebase — BRP is simply absent. Adding
  it is a one-line feature flip, but its coverage of frame-stepping +
  framebuffer-capture is unverified here.
- **Process-spawn precedent that DOES exist:** the compare gates re-exec
  `std::process::Command::new(current_exe())` with sub-flags
  (`vox_gpu_oracle.rs` / `vox_web_parity.rs` compare phases, dispatched from
  `bin/e2e_render.rs:172-205`). The Playwright spec
  `e2e/tests/vox-horizon-parity.spec.ts` spawns `cargo run --bin e2e_render`
  as a subprocess 3× per run. **A test runner spawning the SUT is the same
  pattern, already proven** — but it spawns a *binary*, not an addressable
  RPC server.
- **Async / serialization deps already in `Cargo.toml`:** `serde` +
  `serde_json` (`:91-96`), `crossbeam-channel` (`:183`, wasm32-only),
  `tracing`/`tracing-subscriber`. **No `tokio`, no `interprocess`, no
  `bincode`, no `jsonrpc`/`tarpc`.** An RPC layer needs new deps for transport
  + (likely) async runtime.

---

## In-process functional-control surfaces an RPC layer could wrap

| Surface | Where | What it exposes |
|---|---|---|
| `EditorState` resource + brushes | `editor/mod.rs:57`, `editor/tools.rs:226-277` | Mutable resource; `paint_brush`/`cube_brush`/`sphere_brush(&mut WorldData, pos, radius, ty)`. Input-driven today; the e2e edit gates already call brushes programmatically (`oasis_edit_visual` calls `sphere_brush` mid-run). |
| `WorldData` resource | `world/data.rs` | The voxel world; `set_voxels_batch` / pending-edit batches. The edit gates mutate it via `Commands` systems. |
| `E2eGateMode` / `GridPreset` / `ConstructionConfig` / `TaaConfig` / `GiSettings` resources | `gate.rs:48`, `lib.rs`/`bootstrap.rs` | Per-domain config resources, all `#[derive(Resource)]`, all settable via `insert_resource`. The settings panel already mutates `GiSettings` at runtime via `ResMut`. |
| `E2eOutcome.gate_result` | `driver.rs:263-266` | `Option<Result<(),String>>` verdict resource — the in-process channel a verdict-query RPC would read. |
| `E2eScreenshot` + `shoot_primary_window` | `readback.rs:14-37` | Framebuffer capture trigger + stash. |

**No debug console, no scripted-input system, no command-queue, no event-injection
surface exists.** Input is real `ButtonInput<MouseButton>`/keyboard via Bevy; the
e2e gates bypass input entirely by calling brush fns directly from systems.
An RPC layer wrapping "inject input" would need new event-injection code; an RPC
layer wrapping "call brush / set resource / query resource" reuses the above.

---

## Workspace crate layout

Single-crate workspace: `[workspace] members = ["crates/bevy_naadf"]`
(root `Cargo.toml`). `crates/bevy_naadf` is `lib + cdylib` with 3 binaries
(`bevy-naadf`, `e2e_render`, `bake`). **There is no place for a separate
test-runner crate or RPC-schema crate today** — adding one means adding a
workspace member. A shared RPC-schema crate (consumed by both the SUT and the
runner, language-pinned for wire-format stability) would naturally be a new
`crates/naadf_rpc` (or similar) member. No `tests/` directory exists in any
member; 195 inline `#[cfg(test)]` blocks; `just test` = `cargo test --workspace`.

---

## Top reuse recommendation

**No existing code covers the IPC-RPC transport — that part is genuinely
greenfield (no socket/pipe/BRP code exists, no async runtime dep).** But the
restructure is *not* a from-scratch effort: the single best reuse anchor is
**`bootstrap::BootstrapInputs` + `build_app_with_bootstrap_inputs`
(`bootstrap.rs:48-225`)**. It is already the one funnel where every config
value is fanned into resources before `app.run()`, it already carries the
`E2eGateMode` enum the follow-up doc names as "the natural RPC-controllable
handle," and it is where an RPC `Plugin` would be conditionally installed. The
restructure's shape is: keep `build_app_core` + `BootstrapInputs`, add an RPC
server plugin gated on a new `BootstrapInputs` field (or a cargo feature),
delete `bin/e2e_render.rs`'s 3-layer parser + the `e2e_driver` state machine,
and re-express each gate's phase sequence as an RPC call sequence in a
`tests/` body. The capture (`readback.rs`/`framebuffer.rs`), SSIM
(`ssim.rs`), cross-world-channel (`checks.rs` `PipelineScanResult`), and brush
(`editor/tools.rs`) primitives all survive as the RPC verb implementations.

---

## Borderline calls

- **`e2e_driver` state machine (`driver.rs`) — "not applicable" vs "extract & extend".**
  Verdict landed as *not applicable as-is* because the `match`-over-`E2ePhase`
  orchestration IS the in-app-driver-mode pattern the restructure deletes.
  But it is borderline: the *primitives* it invokes (frame-budget counting,
  `shoot_primary_window`, brush calls, `run_assertions`) are exactly the RPC
  verbs a runner needs. What flips it: if the design chooses "RPC server inside
  the production app exposing tick/capture/edit verbs," then `e2e_driver`'s
  body is the *spec* for those verbs and large parts get extracted, not
  discarded. The architect must decide whether RPC verbs are thin wrappers
  over driver-internal helpers (extract) or a fresh surface (discard the
  orchestration only).

- **`E2eGateMode` enum — "extend" vs "becomes vestigial."**
  Verdict is *extend* because the driver, camera-pin systems, and
  `window_for_gate_mode` all still branch on it. But `04-followup` § "Why this
  is cleaner" says "the app has no notion of e2e_mode" — if the restructure
  fully removes `add_e2e_systems` from the SUT, the driver and `pin_*_camera`
  systems leave with it, and `E2eGateMode`'s only remaining reader is
  `window_for_gate_mode` (window sizing). What flips it: whether the SUT
  retains *any* e2e systems. If it retains zero, `E2eGateMode` shrinks to a
  window-size selector or is replaced by an RPC `resize` verb — i.e. the
  Step-6 enum the in-flight refactor just built could become near-dead. The
  architect must resolve this against the "no e2e modes baked into the app"
  principle.

- **BRP (Bevy Remote Protocol) — un-auditable from code.**
  `bevy_remote` is absent from the dependency tree, so whether BRP's
  entity/component query+mutate surface covers frame-stepping and
  framebuffer-capture (open question #1 of `04-followup`) cannot be answered
  by reading this repo. This is *not applicable* as existing code, but it is
  borderline because if BRP covers the surface, the "custom RPC layer" is
  small; if not, it is large. What flips it: a spike enabling
  `bevy = { features = ["bevy_remote"] }` and probing BRP's verb set —
  outside this audit's scope, but the architect must do it before sizing the
  transport work.

- **`editor/` brushes — "extend" vs "reuse."**
  Landed as *extend*: the brush *functions* (`sphere_brush` etc.) are
  pure `(&mut WorldData, …)` calls reusable verbatim, but `EditorState` +
  `apply_edit_tool` are mouse-input-driven and `add_hud`-gated *off* in the
  e2e config. An RPC `apply_brush` verb reuses the brush fns directly (reuse)
  and bypasses `EditorState`/`apply_edit_tool` entirely — so "extend" is
  generous; for the brush fns alone it is clean reuse. What flips it: whether
  the RPC verb needs `EditorState`'s smoothed-`pos`/stroke semantics (extend
  the resource) or just one-shot brush application (reuse the fn, ignore the
  resource).

---

## Side notes / observations / complaints

- **The brief's "~22 e2e entries" is precise but conflates two species.**
  13 are genuine in-app driver modes (booted window + `e2e_driver` state
  machine); 9 are *not* — they are `MinimalPlugins`-headless `pub fn … ->
  Result` calls (`validate_*`) or pure CPU functions (`ssim_compare_command`,
  the two `compare` orchestrators). The IPC-RPC restructure's actual target is
  the **13 booted gates**. The 9 headless validators should move to `tests/`
  regardless of the RPC direction — that is `03-e2e-as-tests-investigation.md`'s
  separable "Option C," ~200 LOC, zero blast radius, independent of this
  orchestration. Recommend the architect explicitly carve those 9 OUT of the
  RPC scope so the design isn't bloated by problems it doesn't need to solve.

- **Line-number drift in `03-e2e-as-tests-investigation.md` § 1.2.** That doc
  was written before the config-as-resource refactor's final commits. Verified
  divergences at HEAD: `run_vox_gpu_oracle_gpu_phase` is `vox_gpu_oracle.rs:310`
  (doc said 305); `run_vox_gpu_oracle_compare` is `:361` (doc said 346);
  `run_vox_web_parity_compare` is `:228` (doc said 212); `parse_gate_command`
  is `bin/e2e_render.rs:259-330` (doc said 256-324); `run_resize_test` is `:370`
  (doc said 353). The doc's *structure* and gate inventory are accurate; only
  line numbers drifted. Treat that doc's prose as correct, its line refs as
  stale — re-`Grep` before citing.

- **The `Gate` trait + `FrameBudget` + `set_camera_pose` scaffolding
  (`gate.rs:96-181`) is dead code carrying `#[allow(dead_code)]`.** It was
  D6-step-2 scaffolding for a *driver-decomposition* refactor that never
  landed. `04-followup` § "Why this is cleaner" already flags it: under the
  IPC-RPC direction "the half-done `Gate` trait … is no longer load-bearing."
  This is rot — it should be deleted by whichever orchestration touches
  `e2e/` next, IPC-RPC or otherwise. Flagging per the project's
  smell-driven-escape rule.

- **`run_with_app` (`e2e/mod.rs:407`) is a one-line `app.run()` wrapper** with
  a docstring referencing a now-deleted `AppArgs`. Cosmetic, but indicative —
  the e2e module has accreted thin indirection layers (`run_e2e_render` →
  `run_with_app` → `app.run()`) that an RPC restructure would flatten.

- **The brief under-asks on the cross-target dimension.** It frames the
  restructure as native-process-spawn, but `e2e/tests/vox-horizon-parity.spec.ts`
  is a *cross-target* gate (native `e2e_render` PNG vs wasm canvas PNG, SSIM
  compared). If `bin/e2e_render` is deleted, that Playwright spec's subprocess
  invocation breaks. `04-followup` open question #4 covers this, but the audit
  brief's "automate any/all e2e scenarios" should be read as native-only
  unless the architect explicitly addresses how the wasm/Android columns reach
  the same RPC schema. The Android deploy chain (`docs/todo/android-build.md`)
  runs `bevy-naadf`, never `e2e_render` — there is *no* Android e2e today, so
  "all scenarios" on Android is net-new, not a restructure.

- **Determinism risk the restructure must not lose.** The current harness's
  determinism rests on `AppConfig::e2e` flipping `synchronous_pipeline_compilation`,
  fixed 256² window, `WinitSettings{Continuous,Continuous}`, and a
  96-frame GI warmup budget (`e2e/mod.rs:88`). If the SUT is the *production*
  `AppConfig::windowed()` app, none of those are on by default. An RPC
  `configure` verb must be able to set the e2e-determinism knobs *or* the SUT
  needs an RPC-controlled boot config — otherwise frame-numbered assertions
  and SSIM tolerances silently destabilise. This is the load-bearing
  viability question the architect must not defer.
