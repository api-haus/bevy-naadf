# 02 — Design: e2e harness restructure to BRP-controlled production app

> Design group file for the `e2e-ipc-rpc-restructure` orchestration. The
> deliverable below is the implementable plan; the orchestrator reviews it at
> the hard design gate before any implementation begins.

---

## delegate-architect design (2026-05-21)

### 0. What was verified against code (and what flipped)

Before designing, the BRP API was read first-hand from the actual
`bevy_remote 0.19.0-rc.1` crate (downloaded from crates.io and extracted to
`/tmp/bevy_remote_inspect/bevy_remote-0.19.0-rc.1/`, since the feature is OFF
in the project so the crate is not in the local registry cache). Three
load-bearing facts from the survey/audit changed on contact with code:

1. **The survey's "BRP drains one request per frame" claim is WRONG.**
   `process_remote_requests` (`bevy_remote/src/lib.rs:1382-1423`) is a
   `while let Ok(message) = world.resource_mut::<BrpReceiver>().try_recv()`
   loop — it drains the **entire mailbox every frame** in the `RemoteLast`
   schedule. Each `Instant` handler runs synchronously via
   `world.run_system_with(id, params)`. The HTTP transport
   (`http.rs:386-430`) `send`s one `BrpMessage` per HTTP request and blocks on
   `result_receiver.recv().await` for that request's reply. So N concurrent
   HTTP requests in one frame all get serviced in that frame's `RemoteLast`.
   This **removes the survey-flagged "hard 80%"** framing of frame-stepping:
   the awkwardness is not "one request per frame", it is "a request completes
   within the frame it is drained, so a handler cannot itself span N frames
   and reply later" (unless it is a watching handler). The frame-stepping
   design (§4) is built on that corrected model.

2. **ZERO `#[derive(Reflect)]` and ZERO `register_type` / `register_resource`
   calls exist anywhere in `crates/bevy_naadf/src/`.** Verified by
   `grep -rn 'Reflect' --include='*.rs'` (no derive hits) and
   `grep -rn 'register_type\|register_resource'` (no hits). BRP's built-in
   resource verbs (`world.get_resources` / `world.mutate_resources` / …) call
   `get_reflect_resource(&type_registry, …)` (`builtin_methods.rs`
   `process_remote_get_resources_request`) and **fail for any type not in the
   `AppTypeRegistry`**. Therefore the built-in resource verbs are unusable for
   *every* project resource as the codebase stands. This is decisive: the
   custom method set (§3) does the get/set work; we do **not** retrofit
   `Reflect` onto the config-resource graph as a prerequisite (that is a large,
   out-of-scope refactor — `WorldData` alone is a deep non-`Reflect` voxel
   buffer struct).

3. **The custom-method API is world-split, exactly as the survey reported.**
   `RemotePlugin::with_method_main` / `with_method_render` /
   `with_watching_method_main` / `with_watching_method_render` verified at
   `bevy_remote/src/lib.rs:591-669`. The render-world variants only exist /
   only register when the `bevy_render` feature of `bevy_remote` is enabled
   (`lib.rs:856` `#[cfg(feature = "bevy_render")]`), and `bevy_internal`
   already enables `bevy_remote` with `features = ["bevy_asset", "bevy_render"]`
   (`bevy_internal-0.19.0-rc.1/Cargo.toml:675-680`). Good — the render-world
   hook is available.

Other verified facts used below:
- The cargo feature name on the `bevy` umbrella is **`bevy_remote`**
  (`bevy_internal Cargo.toml:145`, `bevy_internal/src/lib.rs:77-78`
  `pub use bevy_remote as remote`). NOT `remote`.
- `RemoteHttpPlugin` is `#![cfg(not(target_family = "wasm"))]`
  (`http.rs:11`), default port `15702`, render-subapp port `15703`
  (`http.rs:52-57`), binds `127.0.0.1` (`http.rs:60`), `.with_port(u16)`
  builder exists (`http.rs:171-176`).
- `RemotePlugin` installs a `RemoteLast` schedule after `Last`
  (`lib.rs:832-835`); the HTTP server task starts in `Startup`
  (`http.rs:140`).
- Project boot funnel: `main.rs:34` `fn main() -> AppExit` → native
  `build_app_with_budget(AppConfig::windowed(), grid_preset)` → `.run()`
  (`main.rs:63-70`); `build_app_with_budget` (`lib.rs:161`) →
  `build_app_with_bootstrap_inputs` (`bootstrap.rs:148`) → `build_app_core`
  (`lib.rs:191`). `--vox <path>` is the only production argv (`main.rs:48-57`).
- The 11 booted-gate `run_*` entry fns are confirmed present in
  `e2e/{vox_horizon_parity,vox_e2e,small_edit_visual,oasis_edit_visual,
  mod,vox_web_parity,vox_gpu_construction,small_edit_repro,vox_gpu_oracle}.rs`.
  `--resize-test` and `--entities` are driven inline from `bin/e2e_render.rs`
  (no `run_*` fn) — 13 booted gates total, matching the brief.
- Brush fns are pure: `sphere_brush(&mut WorldData, Vec3, f32, VoxelTypeId,
  bool)` (`editor/tools.rs:277`), `cube_brush` (`:261`), `paint_brush` (`:226`).
  The driver already calls them mid-run via `Option<ResMut<WorldData>>`
  (`driver.rs:491`, `oasis_edit_visual::apply_erase_brush` at
  `oasis_edit_visual.rs:258-295`).
- Capture is transport-agnostic: `shoot_primary_window(&mut Commands)`
  (`readback.rs:34`), `E2eScreenshot(Option<Image>)` resource (`readback.rs:17`),
  `Framebuffer::from_image` (`framebuffer.rs:152`) + `save_png` (`:374`).
- `AppConfig` has exactly four fields (`app_config.rs:17-33`); the e2e/windowed
  divergence is `add_hud`, `add_free_camera`,
  `synchronous_pipeline_compilation`, `window`, `add_e2e_systems`.

---

### 1. Top-level shape of the restructure

The end state has three pieces:

1. **The production binary `bin/bevy-naadf` gains an optional BRP server.**
   It is the SUT. Booted with a CLI/env opt-in, it (a) installs `RemotePlugin`
   + `RemoteHttpPlugin` with the project's custom domain methods, and (b)
   applies the e2e *determinism profile* (the `AppConfig::e2e()` knobs) — both
   gated by the same spawn-time switch. Booted normally, none of this is
   present and the binary is byte-identical to today.

2. **A new workspace member `crates/naadf_e2e` — the external test runner.**
   A `lib`-only crate (with an integration-test target) that spawns
   `bin/bevy-naadf` as a subprocess, speaks BRP-over-HTTP to it, and re-expresses
   each of the 13 gates as an ordinary `#[test]` body. It owns the BRP client,
   the per-gate scenario logic that used to live in `e2e/driver.rs`'s phase
   machine, and the SSIM/PNG assertions (calling the *library's* pure
   `e2e::ssim` + `e2e::framebuffer` code — those stay in `bevy_naadf`).

3. **`bin/e2e_render` and the in-app driver machinery are deleted.**
   The 3-layer argv parser, `e2e_driver` state machine, `E2ePhase`,
   `E2eState`/`E2eOutcome`, the 6 `pin_*_camera` systems, `add_e2e_systems`'
   driver wiring, and the dead `Gate`/`FrameBudget` scaffolding all go. The
   *primitives* survive (capture, brushes, region gates, SSIM, the
   `PipelineScanResult` cross-world channel) — they become the bodies of the
   custom BRP methods.

The reuse anchor is exactly what the audit named: `BootstrapInputs` +
`build_app_with_bootstrap_inputs` (`bootstrap.rs:48-225`) is the single boot
funnel. The BRP plugin installs there, gated on a new field.

---

### 2. BRP server side

#### 2.1 Cargo feature enablement

Add a crate-level cargo feature `e2e-brp` in `crates/bevy_naadf/Cargo.toml`
`[features]`:

```toml
[features]
default = []
e2e-brp = ["bevy/bevy_remote"]   # native-only control surface for the e2e runner
```

`bevy/bevy_remote` is the umbrella feature (verified §0). It transitively
pulls `bevy_remote` with `bevy_asset` + `bevy_render` (so `with_method_render`
is available) and the `http` default feature (so `RemoteHttpPlugin` exists).

**Why a cargo feature and not always-on:** the HTTP transport drags in
`hyper`/`async-io`/`smol-hyper`/`http-body-util` and opens a listening socket
in `Startup`. The production app must not ship a remote-control socket. A
cargo feature keeps the dependency *and* the code out of the shipped binary
entirely. `cargo build --workspace` (the project verification gate) compiles
the default feature set; the runner builds the SUT with `--features e2e-brp`.

The cdylib/wasm/Android targets never enable `e2e-brp` — native-only is
enforced by the runner only ever passing the feature on native `cargo build`,
and `RemoteHttpPlugin` itself is `cfg(not(wasm))` so even an accidental
wasm build with the feature would just omit the transport.

#### 2.2 Where the BRP plugin installs in the boot path

A new module `crates/bevy_naadf/src/e2e_brp/mod.rs` (entirely behind
`#[cfg(feature = "e2e-brp")]`) exposes one entry point:

```rust
#[cfg(feature = "e2e-brp")]
pub fn install_brp_server(app: &mut App, port: u16) { /* see §2.3 */ }
```

It is called from **`build_app_core` (`lib.rs:191`)**, at the end of the
plugin pyramid, gated on a new field of `AppConfig`:

```rust
// app_config.rs — AppConfig gains one field:
pub struct AppConfig {
    pub add_hud: bool,
    pub add_free_camera: bool,
    pub synchronous_pipeline_compilation: bool,
    pub window: WindowConfig,
    pub add_e2e_systems: bool,        // see §6 — shrinks, not removed yet
    pub brp_port: Option<u16>,        // NEW — Some(port) ⇒ install BRP server
}
```

`AppConfig::windowed()` / `AppConfig::e2e()` set `brp_port: None`.
`build_app_core` ends with:

```rust
#[cfg(feature = "e2e-brp")]
if let Some(port) = cfg.brp_port {
    crate::e2e_brp::install_brp_server(&mut app, port);
}
```

**Why `build_app_core` and not `build_app_with_bootstrap_inputs`:**
`build_app_core` is the *single* funnel both production and e2e route through
(`lib.rs:191` doc + `bootstrap.rs:149`). Installing here means the SUT is the
real `build_app_with_budget(AppConfig::windowed(), …)` production path with
exactly one delta (the BRP plugin), which is the whole point of the
restructure — "the production app is the SUT." `AppConfig` is already the
established "deliberate deltas" carrier (`app_config.rs:11-15`), so one more
field is idiomatic. The plugin must install *after* `RenderPlugin` is built
(the render sub-app must exist for `with_method_render` registration —
`bevy_remote/src/lib.rs:860` `get_sub_app_mut(RenderApp)`); `build_app_core`
adds `DefaultPlugins` (incl. `RenderPlugin`) before its tail, so end-of-funnel
is correct.

#### 2.3 `RemotePlugin` + transport wiring

`install_brp_server` builds the plugin with all custom methods chained, then
adds the HTTP transport:

```rust
#[cfg(feature = "e2e-brp")]
pub fn install_brp_server(app: &mut App, port: u16) {
    use bevy::remote::{RemotePlugin, http::RemoteHttpPlugin};

    let plugin = RemotePlugin::default()           // built-in verbs kept (cheap, harmless)
        // main-world domain methods
        .with_method_main("naadf/step",            verbs::step)
        .with_watching_method_main("naadf/run_until_idle", verbs::run_until_idle)
        .with_method_main("naadf/capture",         verbs::capture_request)
        .with_watching_method_main("naadf/await_capture", verbs::await_capture)
        .with_method_main("naadf/apply_brush",     verbs::apply_brush)
        .with_method_main("naadf/get_state",       verbs::get_state)
        .with_method_main("naadf/set_camera",      verbs::set_camera)
        .with_method_main("naadf/load_world",      verbs::load_world)
        .with_method_main("naadf/region_gate",     verbs::region_gate)
        .with_method_main("naadf/resize_window",   verbs::resize_window)
        // render-world domain method
        .with_method_render("naadf/pipeline_scan", verbs::pipeline_scan);

    app.add_plugins(plugin);
    app.add_plugins(RemoteHttpPlugin::default().with_port(port));
    app.init_resource::<verbs::E2eControl>();      // see §4
}
```

`RemotePlugin::default()` is kept (not `RemotePlugin::empty()`) — the built-in
verbs cost nothing extra and `rpc.discover` is a useful smoke handle. They are
simply unusable against project resources (§0 fact 2); the design does not
*rely* on them.

The HTTP server task starts in `Startup` (`http.rs:140`) and runs on the
`IoTaskPool` — it does not need the app to be focused. But it *does* need the
app to keep ticking for `process_remote_requests` (in `RemoteLast`, after
`Last`) to drain the mailbox. See §2.4.

#### 2.4 The SUT must tick continuously — the determinism profile

The production `AppConfig::windowed()` uses the default `WinitSettings`, which
drops to `reactive_low_power` when unfocused. An unfocused SUT that only ticks
on events would stall the BRP mailbox drain. The current e2e harness already
solves this: `add_e2e_systems` inserts
`WinitSettings { focused_mode: Continuous, unfocused_mode: Continuous }`
(`e2e/mod.rs:242-245`).

So the spawn switch (§5) does two things, *both* required for a usable SUT:

1. **`brp_port: Some(port)`** — installs the BRP server.
2. **the e2e *determinism profile*** — `synchronous_pipeline_compilation:
   true`, the `Continuous`/`Continuous` `WinitSettings`, the fixed window
   size, and `add_hud: false` / `add_free_camera: false`.

These are NOT independent: a BRP-controlled SUT that is not `Continuous` is
broken (mailbox stalls), and a BRP-controlled SUT without
`synchronous_pipeline_compilation` is non-deterministic (frame-numbered
assertions destabilise). The spawn switch therefore selects a **single
bundled "e2e SUT profile"**, not à-la-carte knobs.

Concretely: a new `AppConfig::e2e_sut(port: u16)` constructor — identical to
`AppConfig::e2e()` except `add_e2e_systems: false` (no in-app driver) and
`brp_port: Some(port)`. The `WinitSettings::Continuous` insert moves out of
`add_e2e_systems` into a small unconditional helper that both `e2e_sut` and
(transitionally) `add_e2e_systems` can call — or simply into
`install_brp_server` (the BRP server is the only thing that *needs*
`Continuous`, so co-locating it there is clean).

This is the answer to the audit's load-bearing determinism risk: the
determinism knobs are not lost and are not an RPC verb — they ride the spawn
profile, exactly as Forbidden Move #4 requires.

---

### 3. The custom BRP method set

All custom methods live in `crates/bevy_naadf/src/e2e_brp/verbs.rs`
(`#[cfg(feature = "e2e-brp")]`). Each is an ordinary Bevy system
`fn(In(Option<Value>), &mut World) -> BrpResult` (or the watching variant).
Params/returns are `serde_json::Value`; a shared param/return struct set
(`#[derive(Serialize, Deserialize)]`) is defined alongside and **re-exported
for the runner crate** so the wire schema has one definition (see §7.1).

Method names are namespaced `naadf/<verb>` to avoid colliding with BRP
built-ins (`world.*`, `schedule.*`, `rpc.*`).

| Method | World | Params | Returns | Wraps (file:line) |
|---|---|---|---|---|
| `naadf/load_world` | main | `{ vox_path?: string }` | `null` | sets `Res<GridPreset>` → `GridPreset::Vox{path}` / `Default`; see §3.1 caveat |
| `naadf/set_camera` | main | `{ translation:[f32;3], look_at:[f32;3], up?:[f32;3] }` | `null` | mutates the `Camera3d`'s `Transform` + `PositionSplit` — same write `pin_oasis_camera` does (`oasis_edit_visual.rs:326-328`) |
| `naadf/step` | main | `{ frames: u32 }` | `{ frame: u64 }` (frame count after) | increments `E2eControl.frames_remaining`; see §4 |
| `naadf/run_until_idle` | main (watching) | `{ max_frames: u32, idle_frames: u32 }` | streamed `{ done: bool, frame: u64 }` | counts down a frame budget; see §4 |
| `naadf/apply_brush` | main | `{ kind:"sphere"\|"cube"\|"paint", pos:[f32;3], radius:f32, voxel_type:u32, erase?:bool }` | `{ voxels_delta:i64, blocks_delta:i64, batches:u32 }` | `editor::tools::{sphere_brush,cube_brush,paint_brush}` (`tools.rs:226-281`) on `ResMut<WorldData>` |
| `naadf/capture` | main | `null` | `{ pending: true }` | `readback::shoot_primary_window(&mut Commands)` (`readback.rs:34`) |
| `naadf/await_capture` | main (watching) | `{ as_png?: bool }` | streamed `{ ready:bool, width,height, png_b64? }` | polls `Res<E2eScreenshot>` (`readback.rs:17`); on `Some`, decodes via `Framebuffer::from_image` (`framebuffer.rs:152`), encodes PNG via `save_png`-equivalent in-memory, returns base64 |
| `naadf/region_gate` | main | `{ rect_fracs:[f32;4] }` (operates on last capture) | `{ mean_rgba:[f32;4], luminance:f32 }` | `Framebuffer::region_mean` + `luminance` (`framebuffer.rs:237,277`) |
| `naadf/get_state` | main | `null` | `{ frame:u64, world_loaded:bool, pipeline_errors:string\|null, tracing_errors:u64 }` | reads `E2eControl`, `WorldData` presence, the `PipelineScanResult` main-world handle (`checks.rs:44`), `TracingErrorCounter` |
| `naadf/pipeline_scan` | **render** | `null` | `{ result: "ok" \| {error:string} }` | reads the *render-world* `PipelineScanResult` clone directly (`checks.rs` — render-world half), no cross-world channel needed since the handler runs in the render world |
| `naadf/resize_window` | main | `{ width:u32, height:u32 }` | `null` | mutates the primary `Window`'s `resolution` — replaces the Hyprland `hyprctl` path; see §8 / §10 |

Notes on individual verbs:

- **`naadf/apply_brush`** is the clean-reuse decision on the `editor/` open
  question: it calls the pure brush fns directly with `&mut WorldData` and
  **does not touch `EditorState`**. `EditorState`'s smoothed-pos/stroke
  semantics are mouse-input artefacts; one-shot programmatic brush application
  needs none of it (the current `oasis_edit_visual::apply_erase_brush` already
  bypasses `EditorState` entirely — `oasis_edit_visual.rs:258` calls
  `sphere_brush` straight). The verb returns the voxel/block deltas the
  current edit gates already log (`oasis_edit_visual.rs:271-294`) so a test
  body can assert the producer side cheaply.

- **`naadf/capture` + `naadf/await_capture` are split** because screenshot
  capture is async (the `ScreenshotCaptured` observer fires one-or-more frames
  later — `readback.rs` module doc, `E2E_DRAIN_FRAMES` bound). `capture`
  spawns the screenshot entity and returns immediately; `await_capture` is a
  *watching* method that returns `Ok(None)` (no change) each frame until
  `E2eScreenshot.0` is `Some`, then returns the decoded frame once. The runner
  blocks on that single streamed message. This is the BRP-idiomatic shape for
  "fire async work, wait for completion" and reuses the watching machinery
  (`process_ongoing_watching_requests`, `lib.rs:1427`) rather than inventing a
  poll loop.

- **`naadf/pipeline_scan` is render-world** because `PipelineCache` is a
  render-world resource (`checks.rs` module doc is explicit). The current
  design ferries the scan result main-ward through the `PipelineScanResult`
  `Arc<Mutex>` channel; with `with_method_render` the handler runs *in the
  render world* and reads `PipelineCache` (or the render-world
  `PipelineScanResult` clone) directly. The `Arc<Mutex>` cross-world channel
  can stay (it is cheap and `naadf/get_state` reads its main-world side for a
  combined status) or be dropped in favour of the render-world verb — see
  Decisions §D7.

- The PNG payload travels as **base64 in JSON**. A 256×256 RGBA frame is
  256 KiB raw, ~50-150 KiB as PNG, ~70-200 KiB base64 — well within HTTP/JSON
  tolerances and below any frame-budget concern (capture is not per-frame).
  The 1920×1080 / 1280×720 gates are larger (~2-8 MiB base64) but still a
  once-per-test transfer, not a per-frame readback — it does **not** violate
  the project's per-frame readback budget (that budget is about per-admission
  GiB transfers; a single test-time framebuffer PNG is orthogonal).

---

#### 3.1 `naadf/load_world` and the boot-time-vs-runtime boundary

This is the one verb with a real caveat. `GridPreset` is consumed by
`setup_test_grid` at **`Startup`** (`bootstrap.rs:83-84` doc). A
`naadf/load_world` issued *after* the app is running cannot retroactively
re-run `Startup`.

Two options, and the design picks the second:

- **(rejected) Make `load_world` a true RPC verb that re-installs the world
  at runtime.** This would need a re-runnable world-install path. The voxel IO
  plugin *does* have a runtime `.vox` load path (`poll_pending_vox_parse`,
  drag-and-drop) — but wiring a verb to it is extra surface and a behaviour
  divergence from "the gate loads the fixture the way the binary does."

- **(chosen) The world to load is part of the spawn contract, not a verb.**
  The vox fixture path is passed at spawn (`--vox <path>`, which the production
  binary *already* parses — `main.rs:48`). Each gate's test body spawns the SUT
  with the right `--vox`. `naadf/load_world` is therefore **demoted**: it stays
  in the schema only as an optional *runtime re-load* convenience and most
  gates do not use it. The 13 gates split cleanly: the ones that load Oasis
  spawn with `--vox crates/bevy_naadf/assets/test/oasis_hard_cover.vox`; the
  default-scene / empty-world gates spawn with no `--vox`.

This keeps the verb honest with Forbidden Move #4 (boot-time config goes
through the spawn contract) — `GridPreset` is morally a boot-time knob, and
the binary already has the CLI flag for it. `naadf/load_world` is kept in the
schema (cheap, and a runtime re-load is genuinely useful for an interactive
console) but is **not on the critical path** for the 13 gates.

---

### 4. Frame-stepping design

The corrected model (§0 fact 1): a BRP handler runs to completion *inside* the
frame its request is drained. An `Instant` handler cannot "advance the app N
frames and then reply" — it has `&mut World` for one synchronous call. So
frame-stepping is built from two primitives:

#### 4.1 `E2eControl` — the in-SUT step gate

A small resource installed by `install_brp_server`:

```rust
#[derive(Resource, Default)]
pub struct E2eControl {
    pub frame: u64,              // monotonic frame counter, ++ every Update
    pub frames_remaining: u32,   // step budget; the app "runs" while > 0
    pub paused: bool,            // when true and frames_remaining == 0, idle
}
```

A single `Update` system `advance_e2e_control` (registered by
`install_brp_server`, runs early in `Update`):

```rust
fn advance_e2e_control(mut c: ResMut<E2eControl>) {
    c.frame += 1;
    c.frames_remaining = c.frames_remaining.saturating_sub(1);
}
```

The app **always ticks** (it is `WinitSettings::Continuous` — §2.4); the SUT
does not literally stop. `frames_remaining`/`paused` are a *logical* gate:
the runner treats "the app is at rest" as `frames_remaining == 0`.

#### 4.2 `naadf/step` — advance exactly N frames

`naadf/step { frames: N }` is an `Instant` main-world handler. It does **not**
loop the schedule. It simply adds `N` to `E2eControl.frames_remaining` and
returns the *current* frame number. Because the request is drained in
`RemoteLast` (after `Last`), the reply tells the runner "your N frames are now
queued; frame X is current." The runner then waits for the app to have
advanced N frames before its next assertion — via `naadf/run_until_idle`
(§4.3) or by polling `naadf/get_state` until `get_state.frame >= X + N`.

**Why not a verb that pumps the schedule N times in-handler:** a BRP handler
gets `&mut World`, and `world.run_schedule(Update)` is callable — but pumping
`Update` from inside `RemoteLast` re-enters the schedule mid-frame, runs
`WinitPlugin`-coupled systems out of their event-loop context, and decouples
the rendered frames from the logical step count (the render sub-app extract
would not be paced with it). That is exactly the "in-app driver fighting the
pipeline" smell. Keeping stepping as "the real winit loop ticks; a counter
tracks progress" means every stepped frame is a *real* rendered frame —
identical to how `e2e_driver` counts ticks today (`E2eState.phase_ticks`),
just with the orchestration moved out-of-process.

#### 4.3 `naadf/run_until_idle` — the deterministic "advance then assert" primitive

This is the verb the runner actually uses between scenario steps. It is a
**watching** main-world method:

```
params:  { max_frames: u32, idle_frames: u32 }
returns: streamed Ok(None) each frame while running;
         one final Ok({ done:true, frame:u64 }) when settled or budget hit.
```

Semantics: each frame `process_ongoing_watching_requests` re-runs the handler.
The handler returns `Ok(None)` (no change → no message sent) until either
`E2eControl.frames_remaining == 0` has held for `idle_frames` consecutive
frames, or `max_frames` total elapsed — then it returns the final
`Ok(Some({done:true,...}))`, which the HTTP transport delivers as the last
SSE chunk and the runner's blocking `recv()` resolves.

This gives the runner a single call shape — "advance the app until it is at
rest, then I assert" — with a hard `max_frames` ceiling so a hung SUT fails
fast (memory: e2e gates must fail fast). It maps 1:1 onto the current driver's
"count a fixed frame budget, then ASSERT" (e.g. `OASIS_WARMUP_FRAMES = 120`,
`OASIS_POST_EDIT_WAIT_FRAMES = 300` — `oasis_edit_visual.rs:113,121`): the
test body issues `step{frames:120}` then `run_until_idle{max_frames:160,
idle_frames:8}`.

**Determinism:** every gate's frame counts are reproduced exactly — the test
body issues the same `step` budgets the current per-gate `*_FRAMES` constants
encode. `synchronous_pipeline_compilation` (from the spawn profile, §2.4)
guarantees pipelines resolve same-frame, so frame-numbered assertions are as
stable as today. The watching-method drain order is deterministic
(`process_ongoing_watching_requests` iterates the request vec in order,
`lib.rs:1429`).

#### 4.4 Alternative considered and rejected

The survey floated "the runner issues N no-op calls" — i.e. one HTTP request
per frame to pace stepping. Rejected: it couples test wall-time to HTTP RTT
(thousands of round-trips for a 300-frame wait), and BRP drains the whole
mailbox per frame anyway so N calls in flight do not equal N frames. The
counter-plus-watching-method design advances frames at the SUT's native tick
rate and reports completion in one streamed message.

---

### 5. The spawn-time determinism contract

The runner spawns the SUT with `std::process::Command`. The contract is
**CLI flags on `bin/bevy-naadf`**, parsed in `main.rs` alongside the existing
`--vox`:

| Flag | Effect |
|---|---|
| `--e2e-brp <port>` | selects `AppConfig::e2e_sut(port)` instead of `AppConfig::windowed()`; installs the BRP server on `port`; applies the e2e determinism profile (§2.4) |
| `--vox <path>` | unchanged — already parsed (`main.rs:48`); the gate's world fixture |
| `--e2e-window <w>x<h>` | optional override of the SUT window size (default 256×256 from the e2e profile); the 1920×1080 / 1280×720 gates pass this |

`main.rs` `fn main()` gains a branch: if `--e2e-brp` is present, build via
`build_app_with_budget(AppConfig::e2e_sut(port), grid_preset)` — note **still
`build_app_with_budget`**, so the SUT runs the real production budget probe +
world-install path, diverging only by the `AppConfig` profile. If `--e2e-brp`
is absent, the existing `AppConfig::windowed()` path is byte-identical to
today.

**Why CLI flags and not env vars:** the project already establishes argv as
the boot-config channel for the production binary (`--vox`), `main.rs` already
has a hand-rolled argv scan, and CLI flags are visible in the spawn `Command`
for debugging. Env vars are equally valid (and the runner could set
`RUST_LOG` etc. anyway); CLI is chosen for consistency with the existing
`--vox` precedent. The flag set is deliberately tiny — 3 flags, hand-parsed,
no `clap` (matching `main.rs`'s existing "minimal `std::env::args` parsing —
no `clap`" doctrine, `main.rs:29`).

Determinism knobs reaching the SUT this way (per Forbidden Move #4 — these
are pre-`run()` and cannot be RPC verbs):
- `synchronous_pipeline_compilation: true` — in the `e2e_sut` profile.
- fixed window size — the `e2e_sut` profile default + `--e2e-window` override.
- `WinitSettings::Continuous` — installed by `install_brp_server` (§2.4).
- the 96-frame GI warmup budget — **not a boot knob at all**; it is a *frame
  count* the driver counts. In the new world the test body issues
  `step{frames:96}`. So the GI warmup "knob" simply becomes a runner-side
  constant, no spawn-contract entry needed.

---

### 6. Fate of `e2e_driver`, `E2eGateMode`, `add_e2e_systems`

#### 6.1 `e2e_driver` state machine — **discard the orchestration, the primitives are already extracted**

`e2e/driver.rs` (~1900 lines, `E2ePhase` 26 variants) is the in-app driver
mode the restructure exists to retire. Decision: **delete the whole
orchestration.** Its phase steps become RPC call *sequences* in the runner's
test bodies. Crucially, the primitives it calls are **already separate
functions** and need no extraction work:
- frame counting → `E2eControl` + `naadf/step` (§4).
- `shoot_primary_window` → already a free fn (`readback.rs:34`), wrapped by
  `naadf/capture`.
- brush calls → already free fns (`editor/tools.rs`), wrapped by
  `naadf/apply_brush`. Note `oasis_edit_visual::apply_erase_brush` is a thin
  gate-specific wrapper — its *logic* (pick centre, radius, erase) moves into
  the runner's oasis test body; the verb just takes explicit params.
- `run_assertions` (`driver.rs:1887`) + the region gates (`gates.rs`
  `batch_gate`, `region_luminance_report`, `assert_nodes_dispatched`) → the
  *pure* assertion fns stay in `bevy_naadf` (they take `&Framebuffer` /
  `&DiagnosticsStore`); the runner calls them via `naadf/region_gate` +
  `naadf/get_state`, or re-implements the thin numeric checks runner-side.

So the audit's borderline call resolves as **"discard the orchestration"** —
because extraction is already done by the codebase's own structure. The
~1900-line `match`-over-`E2ePhase` is pure orchestration glue and deletes
wholesale.

#### 6.2 `E2eGateMode` enum — **becomes vestigial; delete it**

Once `add_e2e_systems`' driver + the 6 `pin_*_camera` systems leave the SUT,
`E2eGateMode`'s readers are:
- the deleted `e2e_driver` (gone),
- the 6 `pin_*_camera` systems (gone — camera is now `naadf/set_camera`),
- `window_for_gate_mode` (window sizing — replaced by the `--e2e-window`
  spawn flag, §5),
- `setup_test_grid`'s test-only CPU-oracle install branch
  (`bootstrap.rs:194` doc mentions it) — this needs a check (§ Assumptions),
  but if it is only the oracle-gate path, it folds into `GridPreset` /
  the spawn contract.

With every reader removed, `E2eGateMode` is dead. **Delete it**, and delete
the `gate_mode` field from `BootstrapInputs` (`bootstrap.rs:107`). This
contradicts the audit's *table* verdict ("extend") but matches the audit's own
*Borderline call* ("if the SUT retains zero e2e systems… the Step-6 enum could
become near-dead") and the `04-followup` principle ("the app has no notion of
e2e_mode"). The config-as-resource refactor's Step 6 work is not "wasted" — it
collapsed 10 booleans into 1 enum, which made *this* deletion a one-symbol
removal instead of a 10-field excision. It served as the stepping stone
`04-followup` predicted.

#### 6.3 `add_e2e_systems` / `AppConfig::e2e()` — shrinks to near-nothing, kept transitionally

`add_e2e_systems` (`e2e/mod.rs:226`) currently wires the driver, the camera
pins, the readback resources, the `PipelineScanResult` channel, and
`WinitSettings::Continuous`. After the restructure:
- driver + camera pins → deleted.
- `WinitSettings::Continuous` → moves to `install_brp_server` (§2.4).
- `E2eScreenshot` resource + the `PipelineScanResult` channel + the
  render-world scan system → these are needed by the `naadf/capture` /
  `naadf/pipeline_scan` verbs, so they move into `install_brp_server`'s setup.
- the `LogPlugin` custom layer for `TracingErrorCounter` (`lib.rs:380-387`,
  gated on `add_e2e_systems`) → re-gate on `cfg.brp_port.is_some()`.

Net: `add_e2e_systems` and `AppConfig::e2e()` have no remaining purpose once
all 13 gates are migrated. They are **deleted in the final phase** (§9 Phase
5), not earlier — keeping them lets the legacy `bin/e2e_render` path stay
runnable during migration so the project verification gate
(`cargo run --bin e2e_render -- <mode>`) is not broken mid-restructure (§9).

---

### 7. Test-runner side

#### 7.1 New workspace member: `crates/naadf_e2e`

Add to root `Cargo.toml`: `members = ["crates/bevy_naadf", "crates/naadf_e2e"]`.

`crates/naadf_e2e/` layout:
```
Cargo.toml
src/lib.rs            — the BRP client + SUT-process harness
src/sut.rs            — Sut: spawn/Drop bin/bevy-naadf, port allocation
src/client.rs         — BrpClient: HTTP JSON-RPC, blocking
src/scenario.rs       — high-level helpers: warmup(), brush(), capture(), …
tests/oasis_edit_visual.rs   — one file per gate (13 files)
tests/vox_e2e.rs
tests/... (13 total)
```

**Schema crate?** The brief asks whether a separate BRP-schema crate is
warranted. Decision: **no separate crate.** The param/return structs live in
`bevy_naadf::e2e_brp::schema` (behind `#[cfg(feature = "e2e-brp")]`, but the
*schema* sub-module is compiled unconditionally — only the *handlers* are
feature-gated) and `naadf_e2e` depends on `bevy_naadf` with
`default-features = false` to import them. Rationale: a third crate buys
language-agnostic wire stability, but both ends are Rust in one workspace
pinned to one revision — a shared module is simpler and the wire format is
JSON (self-describing, the Playwright side §8 can hand-roll its handful of
calls). If a non-Rust client ever needs the schema, `rpc.discover` +
`registry.schema` already emit OpenRPC. Revisit only then.

`naadf_e2e`'s deps: `serde`/`serde_json` (wire), a blocking HTTP client
(`ureq` — tiny, blocking, no async runtime; the runner is synchronous test
code), `bevy_naadf` (for the schema structs + the pure `e2e::ssim` /
`e2e::framebuffer` assertion code). For the watching/SSE methods
(`run_until_idle`, `await_capture`) the runner reads the `text/event-stream`
response body line-by-line — `ureq` exposes the response reader; one small
SSE line-parser (`data: <json>\n\n`) handles it.

#### 7.2 The `Sut` harness

```rust
pub struct Sut { child: Child, port: u16, client: BrpClient }

impl Sut {
    /// Spawn bin/bevy-naadf --e2e-brp <port> [--vox <path>] [--e2e-window WxH],
    /// poll the BRP port until rpc.discover answers (bounded ~30s), return.
    pub fn spawn(opts: SutOpts) -> Sut { ... }
    pub fn client(&mut self) -> &mut BrpClient { ... }
}
impl Drop for Sut { fn drop(&mut self) { self.child.kill(); } }   // no orphans
```

Port allocation: bind a `TcpListener` on `127.0.0.1:0`, read the OS-assigned
port, drop the listener, pass that port to the SUT — avoids the 15702 default
colliding when tests run (even though `cargo test` runs separate test
*binaries*, one per file, and each spawns its own SUT, distinct ports keep it
robust).

The SUT is spawned with `cargo run` *or* a pre-built binary path. To keep
tests fast and avoid `cargo`-within-`cargo`, the runner resolves the SUT
binary from `CARGO_BIN_EXE_bevy-naadf` (Cargo sets this env var for
integration tests of the *same* package — but `bevy-naadf` is in a *different*
crate). Since `naadf_e2e` is a separate crate, that env var is not set;
instead `Sut::spawn` shells `cargo build -p bevy-naadf --features e2e-brp
--bin bevy-naadf` once (cached) at first use and then runs the resolved
`target/debug/bevy-naadf`. A `OnceLock` guards the build so the 13 test files
build the SUT once total.

#### 7.3 Worked example — `--oasis-edit-visual` re-expressed

Current gate (`oasis_edit_visual.rs` + driver phases): load Oasis, birdseye
camera, warmup 120 frames, capture A, erase-sphere at world centre r=30, wait
300 frames, capture B, assert mean per-pixel RGB delta over the central
35–65% rect ≥ 8.0.

As a BRP-driven test body in `crates/naadf_e2e/tests/oasis_edit_visual.rs`:

```rust
use naadf_e2e::{Sut, SutOpts, scenario::*};
use bevy_naadf::e2e::framebuffer::{Framebuffer, Rect};

#[test]
fn oasis_edit_visual() {
    // 1. Spawn the production binary as the SUT, Oasis fixture preloaded
    //    via the spawn contract (§3.1, §5).
    let mut sut = Sut::spawn(SutOpts::new()
        .vox("crates/bevy_naadf/assets/test/oasis_hard_cover.vox")
        .window(256, 256));
    let c = sut.client();

    // 2. Birdseye camera over world centre (replaces pin_oasis_camera).
    let world = c.call("naadf/get_state", json!(null)).unwrap();   // world size etc.
    let (cx, cy, cz) = birdseye_pose(world);          // helper, ported from oasis_edit_visual.rs
    c.call("naadf/set_camera", json!({
        "translation": [cx, cy + 250.0, cz],
        "look_at": [cx, cy, cz], "up": [1.0, 0.0, 0.0],
    })).unwrap();

    // 3. Warm up (TAA + GI convergence) — the OASIS_WARMUP_FRAMES budget.
    c.call("naadf/step", json!({ "frames": 120 })).unwrap();
    c.watch("naadf/run_until_idle", json!({ "max_frames": 160, "idle_frames": 8 }));

    // 4. Capture frame A.
    c.call("naadf/capture", json!(null)).unwrap();
    let before: Framebuffer = c.await_capture_png();   // helper: watch await_capture, decode

    // 5. Erase-sphere at world centre — the load-bearing runtime path.
    c.call("naadf/apply_brush", json!({
        "kind": "sphere", "pos": [cx, cy, cz],
        "radius": 30.0, "voxel_type": 0, "erase": true,
    })).unwrap();

    // 6. Wait for the W2→GPU dispatch to propagate — OASIS_POST_EDIT_WAIT_FRAMES.
    c.call("naadf/step", json!({ "frames": 300 })).unwrap();
    c.watch("naadf/run_until_idle", json!({ "max_frames": 360, "idle_frames": 8 }));

    // 7. Capture frame B.
    c.call("naadf/capture", json!(null)).unwrap();
    let after: Framebuffer = c.await_capture_png();

    // 8. Assert — reuse the library's pure assertion code unchanged.
    let rect = Rect::from_fractional(&after, 0.35, 0.35, 0.65, 0.65);
    let delta = region_mean_pixel_delta(&before, &after, rect);
    before.save_png("target/e2e-screenshots/oasis_edit_before.png").ok();
    after.save_png("target/e2e-screenshots/oasis_edit_after.png").ok();
    assert!(delta >= 8.0, "oasis-edit-visual: rect mean RGB Δ {delta:.2} < floor 8.0");

    // 9. Pipeline-error scan — render-world verb.
    let scan = c.call("naadf/pipeline_scan", json!(null)).unwrap();
    assert_eq!(scan["result"], "ok", "pipeline errors: {scan:?}");
}   // Sut::drop kills the subprocess.
```

The phase machine became straight-line test code. The numeric thresholds
(`OASIS_EDIT_DIFF_FLOOR = 8.0`, the 35–65% rect, the frame budgets) port over
as runner-side constants; the *assertion math* (`region_mean_pixel_delta`,
`Framebuffer::region_mean`) is reused from `bevy_naadf` verbatim.

All 13 gates follow this shape. The 3 currently-Layer-1 "compare" gates
(`vox-gpu-oracle`, `vox-web-parity`) — which today spawn subprocesses to
produce two PNGs then SSIM-compare — become **one test body that drives the
SUT twice** (or spawns two SUTs) and calls `bevy_naadf::e2e::ssim` on the two
captures in-process. The subprocess-orchestrator pattern collapses, exactly as
`04-followup` predicted.

---

### 8. Cross-target Playwright gate

`e2e/tests/vox-horizon-parity.spec.ts` is a *cross-target* gate: it spawns
`cargo run --bin e2e_render -- --vox-horizon-native` to produce the native
1280×720 PNG, captures the wasm canvas in Chrome, then `--ssim-compare`s them.
Deleting `bin/e2e_render` breaks two of its three subprocess calls.

Treatment — **the Playwright spec keeps its job but changes how it gets the
native PNG and the SSIM number:**

1. **Native PNG production:** replace the `cargo run --bin e2e_render --
   --vox-horizon-native` subprocess with `cargo test -p naadf_e2e --test
   vox_horizon_native -- --nocapture`. The `vox_horizon_native` test body
   drives the SUT (BRP) to the C# horizon pose at 1280×720 and writes
   `target/e2e-screenshots/vox_horizon_native.png` — same output path the
   Playwright spec already reads. The test produces the PNG as a *side
   effect*; the Playwright spec consumes the file, unchanged.

2. **SSIM compare:** the `--ssim-compare` CLI is the one genuinely
   developer-facing utility in `e2e_render` that has no BRP analogue (it is a
   pure no-App PNG diff). Two sub-options:
   - **(chosen)** Keep `bin/e2e_render` alive **as a ~30-line SSIM-only
     utility binary** — strip it down to just `--ssim-compare`. It is a pure
     `bevy_naadf::e2e::ssim::ssim_compare_command` wrapper (`ssim.rs`), no
     Bevy, no GPU, no parser layers. The Playwright spec's third subprocess
     call (`--ssim-compare a b --ssim-min`) keeps working verbatim.
   - (rejected) Port the SSIM compare into the Playwright spec as Node code —
     duplicates the SSIM implementation in a second language; the project's
     SSIM is tuned (TAA/GI shimmer tolerance) and must not fork.

So `bin/e2e_render` does **not** vanish entirely — it shrinks from a 547-line
3-layer parser to a single-purpose `--ssim-compare` developer utility. That is
the honest treatment: the Playwright cross-target gate is native-process-spawn
*by necessity* (it bridges native↔wasm and Playwright is the wasm controller),
and a pure-CPU PNG-diff CLI is the right tool for that bridge. It is not "e2e
as an in-app mode" — there is no App, no window, no driver. The restructure's
target (the 13 booted-window in-app driver modes) is fully met; the SSIM CLI
is a leaf utility that legitimately survives.

This is native-only as mandated — no BRP reaches the wasm build; Playwright
stays the wasm controller exactly as today.

---

### 9. Implementation phasing plan

Migration is incremental: the legacy `bin/e2e_render` path stays runnable
until the final phase, so the project verification surface is never fully
broken. Each phase has a concrete gate.

**Phase 0 — transport spike (throwaway, ~half day).**
Behind a scratch branch: add the `e2e-brp` feature, install
`RemotePlugin::default() + RemoteHttpPlugin` in `build_app_core` gated on a
temporary flag, boot `bin/bevy-naadf --e2e-brp 15702`, and from a shell hit
`rpc.discover` + `world.query` over HTTP. **Gate:** the SUT answers BRP over
loopback HTTP and keeps ticking while unfocused (confirms §2.4 / §4 model
against the real winit loop). If `Continuous` is not enough to keep the
mailbox draining, this phase surfaces it before any real code lands. Discard
the spike branch; its only output is a yes/no on the transport + a confirmed
plugin install point.

**Phase 1 — BRP server scaffold (no gates migrated yet).**
Land `crates/bevy_naadf/src/e2e_brp/` (feature-gated): `install_brp_server`,
`E2eControl` + `advance_e2e_control`, the `AppConfig::brp_port` field +
`AppConfig::e2e_sut`, the `--e2e-brp` / `--e2e-window` flags in `main.rs`, and
the `naadf/step` + `naadf/run_until_idle` + `naadf/get_state` verbs only.
`bin/e2e_render` + `e2e_driver` **untouched**. **Gate:** `cargo build
--workspace` (default features) compiles; `cargo build -p bevy_naadf
--features e2e-brp` compiles; the legacy `cargo run --bin e2e_render --
--vox-e2e` still passes (proves the production/e2e paths are unbroken).

**Phase 2 — the rest of the verb set + the `naadf_e2e` crate skeleton.**
Land `naadf/capture`, `naadf/await_capture`, `naadf/apply_brush`,
`naadf/set_camera`, `naadf/region_gate`, `naadf/pipeline_scan`,
`naadf/resize_window`, and the `e2e_brp::schema` module. Add the `naadf_e2e`
workspace member: `Sut`, `BrpClient`, scenario helpers. Migrate **one**
representative gate end-to-end — `oasis_edit_visual` (§7.3) — as
`naadf_e2e/tests/oasis_edit_visual.rs`. **Gate:** `cargo test -p naadf_e2e
--test oasis_edit_visual` passes AND the legacy `cargo run --bin e2e_render --
--oasis-edit-visual` still passes — the same gate green on both paths proves
fidelity before bulk migration.

**Phase 3 — migrate the remaining 12 gates.**
One `naadf_e2e/tests/<gate>.rs` per gate, in batches (the 6 single-capture
gates first — simplest; then the edit gates; then the 2 compare gates as
twice-driven SUT bodies; `resize-test` last — it now uses `naadf/resize_window`
instead of `hyprctl`, removing the Hyprland dependency entirely). **Gate per
batch:** each migrated gate green via `cargo test -p naadf_e2e`; the
corresponding legacy `e2e_render` flag still green until its gate is migrated.

**Phase 4 — repoint the Playwright cross-target gate.**
Shrink `bin/e2e_render` to the `--ssim-compare`-only utility (§8). Update
`e2e/tests/vox-horizon-parity.spec.ts` to invoke `cargo test -p naadf_e2e
--test vox_horizon_native` for the native PNG and keep `--ssim-compare` for
the diff. **Gate:** `just test-wasm` (the Playwright suite, headed) passes.

**Phase 5 — delete the legacy harness.**
Remove `bin/e2e_render`'s parser layers (keep only the SSIM utility),
`e2e/driver.rs`, `e2e/gate.rs` (`E2eGateMode`, `Gate` trait, `FrameBudget`),
the `gate_mode` field on `BootstrapInputs`, `add_e2e_systems`,
`AppConfig::e2e()` + `add_e2e_systems` field, `window_for_gate_mode`, the 6
`pin_*_camera` systems, and the per-gate `run_*` boot fns in `e2e/*.rs`
(their *pure* assertion helpers + `birdseye_pose`-style geometry fns move to
`naadf_e2e` or stay as `pub` library fns the runner imports). **Gate:**
`cargo build --workspace`, `cargo test --workspace --lib`, all 13
`cargo test -p naadf_e2e` gates, `just test-wasm` all green. Update
`CLAUDE.md`'s verification-surface section: the e2e gate is now
`cargo test -p naadf_e2e` (the brief notes this restructure changes what "the
e2e gate" means — Phase 5 is where `CLAUDE.md` is updated to say so).

**Verification-surface evolution.** During Phases 1–4 both paths coexist:
`cargo run --bin e2e_render -- <mode>` (legacy) and `cargo test -p naadf_e2e
--test <gate>` (new). Phase 5 retires the legacy path. The forbidden
`cargo run --bin bevy-naadf` smoke is *never* used — and note the new model
makes that temptation moot: the runner spawns `bin/bevy-naadf` as the SUT, but
under deterministic BRP control with hard frame budgets and programmatic
assertions, which is a *gate*, not a smoke.

---

### 10. Deletion / migration ledger

**Deleted outright (Phase 5):**
- `crates/bevy_naadf/src/bin/e2e_render.rs` — the 3-layer argv parser
  (~547 lines). Only `--ssim-compare` survives, as a shrunk utility binary.
- `crates/bevy_naadf/src/e2e/driver.rs` — `e2e_driver`, `E2ePhase`,
  `E2eState`, `E2eOutcome`, `ResizeTestState` (~1900 lines).
- `crates/bevy_naadf/src/e2e/gate.rs` — `E2eGateMode`, the dead `Gate` trait
  + `FrameBudget` + `set_camera_pose` scaffolding (the audit's flagged rot).
- `BootstrapInputs.gate_mode` field (`bootstrap.rs:107`) +
  `run_e2e_render_with_bootstrap_inputs`'s `window_for_gate_mode` call.
- `add_e2e_systems` (`e2e/mod.rs:226`), `AppConfig::e2e()`,
  `AppConfig.add_e2e_systems`, the 6 `pin_*_camera` systems across the gate
  files, `window_for_gate_mode`.
- the per-gate `run_*` boot fns (`run_oasis_edit_visual`, `run_vox_e2e`, …)
  and their gate-specific `BootstrapInputs` builders.

**Migrates into the BRP verbs / runner (kept, relocated):**
- `e2e/readback.rs` (`E2eScreenshot`, `shoot_primary_window`) → wired by
  `install_brp_server`, wrapped by `naadf/capture`+`naadf/await_capture`.
- `e2e/framebuffer.rs` (`Framebuffer`, `Rect`, region/SSIM-feed helpers) →
  stays a `pub` library module; both the verbs and `naadf_e2e` import it.
- `e2e/ssim.rs` (`ssim_compare_command`, `parse_ssim_compare_args`) → stays a
  `pub` library module; the shrunk `e2e_render` utility + `naadf_e2e` use it.
- `e2e/checks.rs` (`PipelineScanResult`, `scan_pipeline_errors_render_system`,
  `assert_nodes_dispatched`) → wired by `install_brp_server`; the render-world
  scan feeds `naadf/pipeline_scan`, the node-dispatch check feeds
  `naadf/get_state`.
- `e2e/gates.rs` region-gate fns (`batch_gate`, `region_luminance_report`,
  `e2e_camera_transform` & friends) → the *pure* fns stay `pub`; the runner
  imports the ones it needs (camera poses, region math).
- per-gate geometry helpers (`oasis_edit_visual::birdseye_pose`,
  `world_centre_voxel`, the assertion fns like `assert_visual_edit_landed`) →
  move to `naadf_e2e` (test-side) or stay as `pub` library fns.
- `editor/tools.rs` brush fns — **unchanged**, wrapped by `naadf/apply_brush`.

**New:**
- `crates/bevy_naadf/src/e2e_brp/{mod,verbs,schema}.rs` (feature-gated).
- `crates/naadf_e2e/` workspace member (runner + 13 test files).
- `AppConfig.brp_port` field + `AppConfig::e2e_sut`.
- `--e2e-brp` / `--e2e-window` flags in `main.rs`.

---

## Decisions & rejected alternatives

- **D1 — Transport: `RemoteHttpPlugin` over loopback, NOT a custom IPC
  transport plugin.** Chosen because it is zero project code, first-party,
  native-only (matches the native-only mandate), and the corrected
  mailbox-drain model (§0 fact 1) means HTTP RTT is not on a per-frame path —
  the runner makes a handful of calls per scenario, not per frame. *Rejected:*
  a custom Unix-socket/stdio transport plugin pushing into `BrpSender`. It is
  a supported seam, but it is net-new code solving a problem we do not have
  (loopback HTTP on 127.0.0.1 is already a local IPC channel with no network
  exposure). *Flips the call:* if a future requirement needs multi-MiB
  framebuffers at high frequency or a binary wire format, a custom transport
  (still under BRP, not a parallel protocol) becomes worth it — but the 13
  gates capture a framebuffer a few times per test, so JSON+base64 over HTTP
  is fine.

- **D2 — The custom method set does ALL get/set work; built-in BRP resource
  verbs are NOT used.** Forced by §0 fact 2: zero `Reflect` derives, zero
  `register_type` calls, so `world.get_resources` & co. fail for every
  project resource. *Rejected:* retrofitting `#[derive(Reflect)]` +
  `register_type` onto the config-resource graph to unlock the built-in verbs.
  That is a large refactor (`WorldData` is a deep non-`Reflect` buffer struct;
  `GiSettings`/`ConstructionConfig`/etc. would each need it) for marginal gain
  — the custom verbs are thin and the runner needs only a handful of typed
  reads anyway. *Flips the call:* if the project independently adopts `Reflect`
  for an editor/inspector, the built-in verbs come for free and some custom
  getters could be dropped — but that is not this restructure's job.

- **D3 — Frame-stepping = a counter resource + a watching `run_until_idle`
  method, NOT a verb that pumps the schedule.** §4. A handler pumping
  `world.run_schedule(Update)` re-enters the schedule mid-frame and decouples
  rendered frames from the step count. The counter model keeps every stepped
  frame a real winit-paced rendered frame — identical semantics to today's
  `E2eState.phase_ticks`, just orchestrated out-of-process. *Rejected also:*
  the survey's "runner issues N no-op calls to pace stepping" — couples test
  time to HTTP RTT and does not even map to frames (the mailbox drains fully
  per frame).

- **D4 — Spawn contract = CLI flags on `bin/bevy-naadf` (`--e2e-brp <port>`,
  `--e2e-window WxH`, existing `--vox`).** Consistent with the existing
  `--vox` precedent and `main.rs`'s "no `clap`" doctrine. *Rejected:* env
  vars (equally valid, but argv is the established channel and visible in the
  spawn `Command`). The determinism knobs ride a *bundled profile*
  (`AppConfig::e2e_sut`) rather than à-la-carte flags, because a BRP SUT that
  is not `Continuous` + `synchronous_pipeline_compilation` is simply broken —
  there is no valid à-la-carte combination.

- **D5 — `e2e_driver` orchestration discarded; `E2eGateMode` deleted.** The
  audit left these as borderline ("extract vs discard", "extend vs
  vestigial"). Resolved to discard/delete because (a) the primitives the
  driver calls are *already* free functions — no extraction work exists to do;
  (b) with the driver + camera-pins + window-sizing all gone, `E2eGateMode`
  has zero readers. Matches `04-followup`'s "the app has no notion of
  e2e_mode" principle. *Flips the call:* if some gate genuinely needs an
  in-SUT multi-frame orchestration that cannot be expressed as a runner-side
  call sequence, a slim driver remnant might survive — but none of the 13
  gates do (every one is warmup→capture→[edit→wait→capture]→assert, all
  straight-line).

- **D6 — `editor/` brushes: reuse the pure fns directly, ignore
  `EditorState`.** `naadf/apply_brush` calls `sphere_brush`/`cube_brush`/
  `paint_brush` with `&mut WorldData`. *Rejected:* extending `EditorState`'s
  smoothed-pos/stroke semantics — those are mouse-input artefacts irrelevant
  to one-shot programmatic application; the current `apply_erase_brush`
  already bypasses `EditorState`.

- **D7 — `naadf/pipeline_scan` is a render-world method reading `PipelineCache`
  directly; the `PipelineScanResult` cross-world `Arc<Mutex>` channel is
  KEPT (not deleted).** The render-world verb makes the scan a direct read.
  But the `Arc<Mutex>` channel is also read by `naadf/get_state`'s combined
  status (a main-world verb), so it stays — it is cheap and it lets a single
  `get_state` call surface pipeline health alongside frame count. *Flips the
  call:* if `get_state` is split so pipeline status is only ever fetched via
  the render-world verb, the channel could be dropped — minor, deferred.

- **D8 — No separate BRP-schema crate; schema lives in
  `bevy_naadf::e2e_brp::schema`.** Both ends are Rust in one pinned workspace;
  a shared module is simpler than a third crate. *Flips the call:* a non-Rust
  client (or a desire to version the wire format independently) would justify
  extracting it — `rpc.discover`/`registry.schema` already cover discovery in
  the meantime.

- **D9 — `bin/e2e_render` is shrunk, not deleted, to keep `--ssim-compare`.**
  The SSIM CLI is a pure no-App PNG diff with no BRP analogue and is consumed
  by the Playwright cross-target gate. Shrinking to a ~30-line single-purpose
  utility is honest; porting SSIM into Node would fork the tuned algorithm.

- **D10 — `resize-test` uses a `naadf/resize_window` BRP verb, dropping the
  Hyprland dependency.** The current gate shells `hyprctl` and bails without
  `HYPRLAND_INSTANCE_SIGNATURE` — machine-specific rot. Mutating the primary
  `Window`'s `resolution` from a BRP handler triggers the same winit resize
  chain the gate exists to exercise, on any platform. *Flips the call:* if the
  resize bug being guarded is specifically a *compositor*-driven resize (not a
  programmatic one), the Hyprland path would be needed — the gate's doc
  (`e2e/mod.rs:144-155`) frames it as a generic resize-blackness repro, so
  programmatic resize should cover it; the migrating impl agent must confirm
  against the gate's assertion intent.

---

## Assumptions made

- **A1 — `bevy_remote 0.19.0-rc.1` is the version that resolves under the
  project's `bevy = "=0.19.0-rc.1"` pin.** Verified `bevy_internal-0.19.0-rc.1`
  depends on `bevy_remote = "0.19.0-rc.1"` (`Cargo.toml:675`) and the crate
  exists on crates.io (downloaded it). Assumed enabling `bevy/bevy_remote`
  resolves cleanly with no version conflict — not test-compiled (no code
  written this dispatch). Phase 0 spike confirms.

- **A2 — `WinitSettings { Continuous, Continuous }` is sufficient to keep the
  BRP mailbox draining on an unfocused/background SUT.** The current e2e
  harness relies on exactly this for its frame budget; assumed it carries over
  to a BRP-controlled SUT. Phase 0 spike explicitly gates this.

- **A3 — `setup_test_grid`'s `E2eGateMode` reader (mentioned in
  `bootstrap.rs:194` doc as "the test-only CPU-oracle install branch") is only
  the oracle-gate path** and folds into `GridPreset` + the spawn contract when
  `E2eGateMode` is deleted. I did not read `voxel/grid.rs` `setup_test_grid` in
  full — the migrating impl agent MUST verify this before deleting
  `E2eGateMode`; if `setup_test_grid` branches on `E2eGateMode` for something
  other than the oracle path, that branch needs a replacement signal (likely a
  dedicated marker resource set by the spawn contract).

- **A4 — The 256 KiB–8 MiB base64 PNG payloads over loopback HTTP do not
  stress the SUT's frame budget.** Capture is a few-times-per-test operation,
  not per-frame; the project's per-frame readback budget is about
  per-admission GiB transfers and is orthogonal. Assumed; not measured.

- **A5 — `cargo test`'s one-binary-per-`tests/`-file model gives each of the
  13 gates its own process** so each spawns its own SUT subprocess on its own
  port with no shared-winit contention. This is standard Cargo behaviour;
  assumed it holds. The `OnceLock`-guarded SUT build (§7.2) is per test
  *binary* — actually each test binary is a separate process so each rebuilds;
  `cargo build` is incremental so the second+ are near-instant. Acceptable.

- **A6 — A blocking HTTP client (`ureq`) can read BRP's `text/event-stream`
  SSE responses for the watching methods.** `ureq` exposes the response body
  as a `Read`er; an SSE `data: <json>\n\n` line-parser over that reader is
  small. Assumed workable; if `ureq`'s body handling fights chunked SSE, a
  different tiny client (or raw `TcpStream` + manual HTTP/1.1) is the
  fallback — flagged for the runner impl agent.

- **A7 — The `e2e_brp::schema` sub-module can be compiled unconditionally
  (only the *handlers* feature-gated) so `naadf_e2e` can import the
  param/return structs without `bevy_naadf` being built with `e2e-brp`.**
  Plain `serde` structs with no `bevy_remote` dependency — assumed trivially
  true.

- **A8 — All 13 gates' scenario logic is expressible as a straight-line
  runner-side call sequence** (warmup→capture→[edit→wait→capture]→assert). The
  audit + the `03` investigation describe every gate this way; I read
  `oasis_edit_visual.rs` in full and the driver's phase structure, and they
  confirm it. Not every one of the 13 gate files was read end-to-end — the
  migrating impl agent verifies per gate.

---

## Side notes / observations / complaints

- **The survey's "hard 80% is frame-stepping" framing was built on a wrong
  fact and should not anchor the implementation.** `process_remote_requests`
  drains the *whole* mailbox per frame (`bevy_remote/src/lib.rs:1382`) — the
  survey's "one request per frame" is incorrect. Frame-stepping is still a
  real design question, but it is *not* hard: a counter resource + a watching
  method is ~40 lines. The genuinely load-bearing finding is the **`Reflect`
  one** — zero `Reflect` derives in the whole crate — which the survey flagged
  as "unconfirmed, must grep" and which I confirmed. That fact, not
  frame-stepping, shapes the verb set. An impl agent who reads only the survey
  would size the work wrong in both directions.

- **The codebase is in good shape for this restructure — the primitives are
  already clean free functions.** Brushes (`editor/tools.rs`), capture
  (`readback.rs`), framebuffer decode (`framebuffer.rs`), SSIM (`ssim.rs`),
  the cross-world channel (`checks.rs`) are all transport-agnostic, pure, and
  reusable verbatim. The rot is concentrated entirely in the *orchestration*:
  the 1900-line `e2e_driver` `match` and the 547-line 3-layer argv parser.
  This is the good case — the foundation is fine, only the in-app-driver-mode
  glue is wrong, and that glue is exactly what BRP-out-of-process replaces.
  Iterating inside `e2e/` is *not* the wrong move here; the restructure is
  well-scoped and the design is not fighting the foundation.

- **The dead `Gate` trait + `FrameBudget` + `set_camera_pose` scaffolding in
  `e2e/gate.rs` (the audit flagged it, `04-followup` flagged it) finally dies
  in Phase 5.** It was D6-step-2 scaffolding for a driver-decomposition
  refactor that never landed. No special handling needed — it just goes with
  `e2e/gate.rs`.

- **`run_with_app` (`e2e/mod.rs:407`) is a one-line `app.run()` wrapper** —
  cosmetic indirection. It vanishes with the per-gate `run_*` fns. Not worth a
  separate cleanup.

- **The brief's "13 booted gates" count is accurate**, but two of them
  (`--resize-test`, `--entities`) have no `run_*` fn — they are driven inline
  from `bin/e2e_render.rs`. The migrating impl agent should not look for
  `e2e/resize_test.rs` (there is none); the resize logic is in
  `bin/e2e_render.rs::run_resize_test` + the driver's resize phases, and
  `--entities` is the `EntitiesBoot` arm. Both still migrate to
  `naadf_e2e/tests/` cleanly — `resize-test` via `naadf/resize_window` (D10),
  `--entities` via the existing `--entities`-equivalent
  `ConstructionConfig`/`SpawnTestEntity` resources set at spawn time (those
  *are* boot-time, so they ride the spawn contract — possibly needing a
  small `--e2e-entities` spawn flag; the impl agent sizes that).

- **One real risk the phasing mitigates but the orchestrator should watch:**
  the SUT is the *production* `bin/bevy-naadf` with the production budget
  probe (`build_app_with_budget` runs `probe_and_select` — `lib.rs:162`). The
  current e2e harness *deliberately skips* the budget probe ("e2e gates need
  canonical world / TAA for deterministic SSIM" — `lib.rs:159`). On a desktop
  with a ≥1.35 GiB cap the probe picks canonical defaults (byte-identical to
  the skip), so on the dev machine this is a non-issue — but on a constrained
  CI runner the probe could pick mobile rungs and destabilise SSIM. The
  `e2e_sut` profile should consider **forcing canonical budget** (skip the
  probe, like `e2e_render` does today) rather than running it. I did not fold
  this into the design body because it depends on where the gates run; flag it
  to the orchestrator as a decision for the spawn profile — "SUT runs the
  production probe" vs "SUT forces canonical budget for determinism." The
  latter is safer and matches the current harness's stated rationale.

- **`bevy_brp_extras` was checked: it is a Bevy-0.18 crate, no confirmed
  0.19-rc.1 release.** Per the brief, not hard-depended on. It is also
  unnecessary — its `screenshot` method is exactly what `e2e/readback.rs`
  already does, and its input-injection methods are not needed (the gates use
  programmatic brush calls, not synthetic input events). The custom verb set
  fully covers the surface.

- **`crates/naadf_e2e` as a non-`bevy_naadf`-binary test crate means
  `CARGO_BIN_EXE_*` is not set** — the runner has to locate/build the SUT
  binary itself (§7.2). This is mild friction. An alternative the orchestrator
  could prefer: put the `tests/` files *inside* `crates/bevy_naadf/tests/`
  (Cargo sets `CARGO_BIN_EXE_bevy-naadf` for same-package integration tests,
  removing the build-resolution dance) and keep only the BRP-client library in
  a separate crate. I designed the separate crate for cleaner separation, but
  same-package `tests/` is a legitimate simpler shape — flagging it as a fork
  the orchestrator may want to take.
