# 03 — Implementation log: e2e harness restructure to BRP-controlled production app

> Per-phase implementation log for the `e2e-ipc-rpc-restructure` orchestration.
> Each phase appends its section here. All paths are from the worktree root
> `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build` (branch
> `feat/android-build`).

---

## Phase 0 — transport spike (2026-05-21)

**Verdict: both load-bearing assumptions CONFIRMED. The transport model holds.
Keep the Phase 0 edits as the seed of Phase 1.** One design correction for
Phase 1 (built-in BRP verb namespace) and one operational note (SUT subprocess
CWD) below.

### A1 result — `bevy/bevy_remote` resolves cleanly against `=0.19.0-rc.1`

**CONFIRMED, no version conflict.**

- Before the spike, `bevy_remote` was absent from `Cargo.lock`
  (`grep 'name = "bevy_remote"' Cargo.lock` → 0 matches).
- Adding the crate-level feature `e2e-brp = ["bevy/bevy_remote"]` and running
  `cargo build -p bevy-naadf --features e2e-brp --bin bevy-naadf` resolved
  `bevy_remote v0.19.0-rc.1` (exact match to the `bevy = "=0.19.0-rc.1"` pin),
  pulled the HTTP-transport dependency chain (`hyper`, `smol-hyper`,
  `async-io`, `http-body-util`, `async-channel` — all verified present in
  `Cargo.lock` after the build), and compiled to completion in ~4m30s with
  **zero errors, zero version-resolution conflicts**.
- `cargo build --workspace` (default features, no `e2e-brp`) recompiled
  `bevy-naadf` green in ~55s — confirming all BRP code is correctly behind
  `#[cfg(feature = "e2e-brp")]` and the default production build is unaffected.

The design's §0 fact ("`bevy_internal-0.19.0-rc.1` depends on `bevy_remote =
"0.19.0-rc.1"`") is borne out by the actual build. A1 is closed.

### A2 result — `Continuous`/`Continuous` keeps the BRP mailbox draining unfocused

**CONFIRMED.** The BRP server answers over loopback HTTP, and it keeps
servicing requests over time *including while the SUT window is genuinely
unfocused*.

Evidence — on-disk capture: `docs/orchestrate/e2e-ipc-rpc-restructure/03-phase0-brp-capture.log`
(full request/response transcript). Driver script: `/tmp/e2e-brp-spike/drive.sh`.

**(a) The SUT answers BRP over loopback HTTP.** Booted
`bin/bevy-naadf --e2e-brp 15702`; the BRP HTTP server answered `rpc.discover`
~1s after boot. All three probed built-in verbs returned a JSON-RPC `result`:

- `rpc.discover` → advertised 23 methods, OpenRPC 1.3.2, server
  `127.0.0.1:15702`, `"title":"Bevy Remote Protocol","version":"0.19.0-rc.1"`.
- `world.list_resources` → returned a 39-element list of registered resource
  type names.
- `world.query` over `bevy_window::window::Window` → returned the live
  reflected `Window` component (resolution, title `"bevy-naadf"`, focus state,
  …) for the real SUT window entity.

Sample request/response pair (from the capture log):
```
REQUEST : {"jsonrpc":"2.0","id":1,"method":"rpc.discover"}
RESPONSE: {"jsonrpc":"2.0","id":1,"result":{"info":{"title":"Bevy Remote Protocol",
           "version":"0.19.0-rc.1"},"methods":[...23 methods...],
           "openrpc":"1.3.2","servers":[{"name":"Server","url":"127.0.0.1:15702"}]}}
```

**(b) It keeps servicing requests over ~15s while unfocused.** The driver
backgrounded the SUT (Hyprland `dispatch workspace e+1` — moved the workspace
away from the SUT window) and then issued `rpc.discover` once per second for
~15s: **14/14 requests answered `OK`** (`result` present, `curl` rc=0) over
the whole span — see the `t+0s … t+14s` lines in the capture log.

To rule out a false positive on "unfocused", a second tighter run polled the
SUT's *own self-reported focus state* via `world.query` during the backgrounded
phase. The SUT reported:
```
at-boot focused = True
after workspace switch:  t+1s..t+8s  SUT reports window.focused = False  (every poll)
```
The SUT answered every `world.query` request during the 8s window it was
reporting `focused = false`. The `world.query` JSON in the capture log
likewise shows `"focused":false` for the post-switch calls. So the BRP mailbox
(`process_remote_requests`, `RemoteLast` schedule) demonstrably keeps draining
on a `WinitSettings { Continuous, Continuous }` SUT whose window is genuinely
unfocused — exactly what A2 needed. A2 is closed.

The corrected mailbox-drain model from design §0 fact 1 ("drains the whole
mailbox per frame") is consistent with the observation: the SUT was answering
~1 request/s comfortably; nothing stalled.

### What was changed

Minimal, contained edits — three files, plus one new feature-gated module.

1. **`crates/bevy_naadf/Cargo.toml`** — added the crate-level feature
   `e2e-brp = ["bevy/bevy_remote"]` to `[features]`, with a doc comment
   explaining the native-only/opt-in rationale (design §2.1).

2. **`crates/bevy_naadf/src/e2e_brp/mod.rs`** (NEW, entirely behind
   `#[cfg(feature = "e2e-brp")]`) — exposes
   `pub fn install_brp_server(app: &mut App, port: u16)`. It:
   - inserts `WinitSettings { focused_mode: Continuous, unfocused_mode:
     Continuous }` (the A2-critical knob),
   - adds `RemotePlugin::default()` (built-in verbs kept — design §2.3),
   - adds `RemoteHttpPlugin::default().with_port(port)`.
   For the Phase 0 spike it installs *only* the default verb set + transport;
   no custom `naadf/*` methods (those are Phase 1+).

3. **`crates/bevy_naadf/src/lib.rs`** — added `#[cfg(feature = "e2e-brp")] pub
   mod e2e_brp;` to the module list, and at the **end of `build_app_core`**
   (the design's specified install point, §2.2) added a feature-gated block:
   ```rust
   #[cfg(feature = "e2e-brp")]
   if let Ok(port) = std::env::var("BEVY_NAADF_E2E_BRP_PORT") {
       if let Ok(port) = port.parse::<u16>() {
           crate::e2e_brp::install_brp_server(&mut app, port);
       }
   }
   ```
   This sits after `DefaultPlugins` (incl. `RenderPlugin`), so the render
   sub-app exists — required for Phase 1's `with_method_render` registration.

4. **`crates/bevy_naadf/src/main.rs`** — added a minimal `--e2e-brp <port>`
   argv parse alongside the existing `--vox` scan; it `std::env::set_var`s
   `BEVY_NAADF_E2E_BRP_PORT`, which `build_app_core` reads. The production
   binary still routes through `build_app_with_budget(AppConfig::windowed(),
   …)` — the spike does NOT introduce `AppConfig::e2e_sut` or the determinism
   profile (Phase 1+ scope).

**Install point used:** end of `build_app_core` (`lib.rs`), exactly as design
§2.2 specifies. Confirmed correct.

**Temp opt-in mechanism:** `--e2e-brp <port>` argv flag in `main.rs` → sets
`BEVY_NAADF_E2E_BRP_PORT` env var → read by the `#[cfg(feature = "e2e-brp")]`
block in `build_app_core`. SPIKE-grade: the design's Phase 1 replaces both the
env-var bridge and the argv flag with an `AppConfig::brp_port: Option<u16>`
field set by `AppConfig::e2e_sut(port)`. The env-var indirection exists only
because the spike deliberately does not touch `AppConfig` (a Phase 1 change).

### Default-build integrity

**`cargo build --workspace` (no `e2e-brp`) compiles green** — verified after
the feature was added (recompiled `bevy-naadf` in ~55s, 0 errors). All BRP
code (`e2e_brp` module, the `build_app_core` install block, the `e2e_brp`
`pub mod` line) is behind `#[cfg(feature = "e2e-brp")]`. The `--e2e-brp` argv
parse in `main.rs` is *not* feature-gated — but with the feature off it only
sets an env var that nothing reads, so the production binary's behaviour is
byte-identical. (Leaving the flag parse un-gated is intentional: it keeps
`main.rs` from needing a `cfg` and means a default-build binary fails cleanly
with a clear error if someone passes `--e2e-brp` without a port, rather than
silently ignoring a typo'd flag.)

### Recommendation

**Keep the Phase 0 edits as the seed of Phase 1.** They are clean, minimal,
correctly feature-gated, and the install point is confirmed correct. Phase 1
builds directly on `e2e_brp::install_brp_server` — it adds the custom `naadf/*`
verbs to the `RemotePlugin` builder inside that function and replaces the
env-var opt-in with the `AppConfig::brp_port` field.

**Corrections / notes for Phase 1 to incorporate:**

1. **DESIGN CORRECTION — built-in verb namespace.** Design §9-Phase-0 says to
   hit "`rpc.discover` + `world.query`", which is correct. But the design's
   §2 prose and the audit/survey elsewhere mention BRP verbs like `bevy/list`
   and `bevy/query` (the *old* 0.15-era namespace). **In `bevy_remote
   0.19.0-rc.1` the built-in verbs are `world.*` / `schedule.*` /
   `registry.*` / `rpc.*`** — confirmed by `rpc.discover`'s own method list
   (the 23 names are all in `03-phase0-brp-capture.log`). `bevy/list` and
   `bevy/query` return JSON-RPC error `-32601 "Method not found"`. The
   custom-method names in design §3 are already correctly namespaced
   `naadf/*`, which avoids any collision — so this only matters for the
   Phase 0 smoke verbs and for any Phase 1+ test that uses a built-in verb.
   Phase 1's `BrpClient` and any built-in-verb calls must use `world.query`,
   `world.list_resources`, etc.

2. **OPERATIONAL NOTE — SUT subprocess CWD.** When `bin/bevy-naadf` is run
   directly from `target/debug/` (as the spike did, and as the Phase 1+
   `Sut::spawn` harness will), Bevy's `AssetPlugin { file_path: "src/assets" }`
   resolves shaders *relative to the process CWD*. Running the binary with CWD
   ≠ the crate root produces a wall of `bevy_asset::server: Path not found:
   .../src/assets/shaders/*.wgsl` errors and the renderer never produces a
   real frame. This is pre-existing behaviour (not introduced by the spike,
   and irrelevant to the *transport* — BRP answered fine regardless), but
   Phase 1's `Sut::spawn` **must set the subprocess `current_dir` to the
   `bevy_naadf` crate root** (or run via `cargo run`, which does this) or the
   capture/render-dependent verbs will see a blank renderer. Flag this for the
   Phase 1/2 runner-harness work — it is load-bearing for `naadf/capture`.

3. The design's `WinitSettings::Continuous` placement decision (§2.4 — "into
   `install_brp_server`") is confirmed sound: co-locating it there is exactly
   what made A2 pass, and it is the single thing that needs `Continuous`.

---

## Side notes / observations / complaints

- **The spike was clean and the design is well-grounded.** Both load-bearing
  assumptions held on first contact with the running engine; no rework of the
  frame-stepping design (§4) is implied. The design's §0 "what was verified
  against code" section is accurate where the spike could check it (the
  `bevy_remote` version, the umbrella feature name `bevy_remote`, the
  `RemoteHttpPlugin` port/loopback behaviour, the `RemoteLast` mailbox-drain
  model). The architect did the homework.

- **The one stale fact is the verb namespace** (correction #1 above). The
  design *body* (§3 method table) is unaffected because the custom verbs are
  `naadf/*`. But the orchestrator should be aware that any place in the
  context bundle / survey / audit that says `bevy/query` or `bevy/list` is
  citing a pre-0.19 BRP. The live `rpc.discover` output
  (`03-phase0-brp-capture.log`) is now the canonical list of built-in verbs
  for this Bevy version — Phase 1+ agents should read it rather than trusting
  the survey's verb names.

- **`world.query` / `world.list_resources` *did* answer** despite the design's
  §0 fact 2 ("zero `#[derive(Reflect)]`, zero `register_type` in the project").
  That fact is still correct and still decisive — but note what the spike
  showed: the built-in verbs work fine for the types *Bevy itself* registers
  (`Window`, `Time`, `ClearColor`, the 39 resources `world.list_resources`
  returned are all `bevy_*` crate types). The design's §0-fact-2 conclusion
  ("built-in resource verbs unusable for *every project resource*") stands —
  `WorldData`, `GiSettings`, `GridPreset` et al. are not in that list because
  the project never `register_type`s them. So the custom `naadf/*` verb set is
  still required for all project state. No change to the design; just a
  precise restatement: built-in verbs are usable for Bevy-owned reflected
  types, unusable for project-owned non-`Reflect` types.

- **The HTTP transport pulled a non-trivial dependency tail** (`hyper`,
  `smol-hyper`, `async-io`, `http-body-util`, `async-channel`, plus their
  transitive deps — ~30+ crates, first build ~4m30s). This is exactly why the
  design made `e2e-brp` an opt-in cargo feature rather than always-on, and the
  spike confirms that was the right call: none of this lands in the default
  `cargo build --workspace` production binary. No concern — just noting the
  build-time cost is real, so Phase 1+'s `Sut::spawn` `OnceLock`-guarded build
  (design §7.2) matters for test wall-time.

- **No subjective red flags.** The restructure's foundation is sound, the
  transport model is confirmed, and the Phase 0 edits are a clean seed. The
  only thing I'd push the orchestrator to make sure Phase 1 does not skip is
  the **SUT CWD** note (#2) — it is the kind of thing that silently makes
  `naadf/capture` return a blank frame and costs a debugging session if the
  runner harness gets it wrong. It is not a design flaw, just an integration
  detail the design does not currently mention.

---

## Phase 1 — BRP server scaffold (2026-05-22)

**Verdict: Phase 1 lands clean. All three verification gates green. The
default-feature production binary is byte-identical to today; the `e2e-brp`
build compiles; the legacy `e2e_render --vox-e2e` gate still passes. The three
Phase-1 verbs answer correctly over loopback HTTP (optional sanity check
performed, results below).**

### What changed

Five files — three edited, two written (one of which was the Phase 0 seed,
now rewritten into the real scaffold).

1. **`crates/bevy_naadf/src/app_config.rs`** — added the
   `AppConfig::brp_port: Option<u16>` field (design §2.2); set it to `None` in
   both existing constructors (`windowed()`, `e2e()`); added the new
   `AppConfig::e2e_sut(port: u16)` constructor (design §2.4 / §5) — the e2e
   determinism profile (HUD off, free camera off, synchronous pipeline
   compilation, fixed 256×256 window) with `add_e2e_systems: false` (no in-app
   driver — the SUT is driven externally over BRP) and `brp_port: Some(port)`.

2. **`crates/bevy_naadf/src/e2e_brp/mod.rs`** — rewrote the Phase 0 spike
   `install_brp_server` into the real scaffold (design §2.3): `RemotePlugin::default()`
   with the three custom verbs chained (`with_method_main` ×2 +
   `with_watching_method_main` ×1), `RemoteHttpPlugin::default().with_port(port)`,
   `WinitSettings::Continuous` (§2.4, A2), `E2eControl` + `RunUntilIdleWatch`
   resources via `init_resource`, and the `advance_e2e_control` system in
   `Update`. Added `pub mod verbs;`.

3. **`crates/bevy_naadf/src/e2e_brp/verbs.rs`** (NEW) — the three Phase-1
   verbs + their support types: `E2eControl` (frame counter + step budget),
   `RunUntilIdleWatch` (single-slot per-watch state), `advance_e2e_control`
   (the `Update` ticker), and `step` / `run_until_idle` / `get_state`.

4. **`crates/bevy_naadf/src/lib.rs`** — replaced the Phase 0 temporary
   `BEVY_NAADF_E2E_BRP_PORT` env-var gate at the end of `build_app_core` with
   the design's `if let Some(port) = cfg.brp_port` field gate (still
   `#[cfg(feature = "e2e-brp")]`, still the same install point). The module
   doc comment for `e2e_brp` is unchanged (already generic).

5. **`crates/bevy_naadf/src/main.rs`** — replaced the Phase 0 env-var bridge
   with the real `--e2e-brp <port>` / `--e2e-window <w>x<h>` flags (design §5),
   moved into the `not(target_arch = "wasm32")` block (native-only spawn
   contract). `--e2e-brp` now selects `AppConfig::e2e_sut(port)` and boots via
   the bootstrap fan-out directly (see budget handling below). Added the
   `parse_window_spec` helper (native-only). Updated the file's `## CLI flags`
   doc section.

`bin/e2e_render`, `e2e/driver.rs`, `e2e/gate.rs`, `E2eGateMode`,
`add_e2e_systems` — all UNTOUCHED, as the brief mandates.

### The three verbs

All three are **main-world** handlers (`with_method_main` /
`with_watching_method_main`, verified against `bevy_remote 0.19.0-rc.1`
`src/lib.rs:591,632`). They are ordinary Bevy systems with the BRP handler
shape — `fn(In(Option<Value>), &mut World) -> BrpResult` for the two instant
verbs, `-> BrpResult<Option<Value>>` for the watching one.

**The `E2eControl` mechanics.** `E2eControl { frame: u64, frames_remaining:
u32 }` is the in-SUT frame-stepping gate (design §4.1). `advance_e2e_control`
runs once per `Update` — `frame += 1`, `frames_remaining =
saturating_sub(1)`. The SUT always ticks (it is `WinitSettings::Continuous`);
`frames_remaining` is a *logical* step budget — "at rest" ⇔
`frames_remaining == 0`. Every counted frame is a genuine winit-paced rendered
frame; the design's D3 decision (counter + watching method, not a
schedule-pumping handler) is followed verbatim.

- **`naadf/step`** (instant) — parses `{ frames: u32 }`, `saturating_add`s it
  to `E2eControl.frames_remaining`, returns `{ frame: u64 }` (the frame count
  *now*, before the queued frames elapse). It does NOT pump the schedule. A
  missing/non-integer `frames` field returns JSON-RPC `-32602 Invalid params`.

- **`naadf/run_until_idle`** (watching) — parses `{ max_frames: u32,
  idle_frames: u32 }`. `process_ongoing_watching_requests` re-runs the handler
  every frame; per the verified `bevy_remote` contract (`src/lib.rs:1431-1435`)
  `Ok(None)` sends nothing (runner keeps blocking), `Ok(Some(v))` delivers `v`
  as the next SSE chunk, `Err` delivers an error chunk. The handler returns
  `Ok(None)` every frame while running, and exactly one
  `Ok(Some({ done: true, frame, timed_out }))` once either `frames_remaining
  == 0` has held for `idle_frames` consecutive frames (`timed_out: false`) or
  `max_frames` frames have elapsed since the watch began (`timed_out: true` —
  the hard ceiling so a hung SUT fails fast, per the e2e-fail-fast memory).

  **The watching method for `run_until_idle`** — a watching handler has no
  per-request storage in the `World`, so the "consecutive idle" / "frames
  since watch began" counters live in the `RunUntilIdleWatch` resource
  (single-slot: `started_at_frame: Option<u64>` + `consecutive_idle: u32`).
  On the first run of a watch the slot is anchored to the current frame; once
  the watch settles or times out the slot is cleared so the next
  `run_until_idle` anchors fresh. Phase 1's runner issues one `run_until_idle`
  at a time (synchronous test code), so a single slot is correct; a
  concurrent-watch design is explicitly out of Phase 1 scope and documented as
  such in the verb's doc comment. If a fresh watch observes a stale slot it
  takes it over (last-writer-wins) — benign for the one-at-a-time runner.

- **`naadf/get_state`** (instant) — ignores params, returns
  `{ frame, frames_remaining, world_loaded, pipeline_errors, tracing_errors }`.
  `world_loaded` = `world.contains_resource::<WorldData>()`.
  `tracing_errors` = `e2e::tracing_error_counter::tracing_error_count()` (a
  process-global static — always readable). `pipeline_errors` reads the
  main-world side of the `PipelineScanResult` `Arc<Mutex>` channel *if the
  resource is present* (`get_resource`, Option-tolerant); in Phase 1 it is
  always `null` because that channel is wired by `add_e2e_systems` (off in
  the `e2e_sut` profile) and the render-world `naadf/pipeline_scan` verb that
  feeds it is Phase 2. `null` here means "not scanned", not "no errors" —
  documented in the verb. Self-contained: Phase 1 did NOT need to wire the
  `PipelineScanResult` channel into `install_brp_server`.

### `e2e_sut` budget handling

The brief's hard-gate resolution is binding: **the e2e SUT forces the
canonical memory budget — it does NOT run the production `probe_and_select`.**

Verified mechanism for how the legacy `e2e_render` path skips the probe:
`run_e2e_render` (`lib.rs:535`) → `e2e::run_e2e_render` (`e2e/mod.rs:398`) →
`build_app(AppConfig::e2e())` (`e2e/mod.rs:399`). `build_app` (`lib.rs:142`)
calls `build_app_core` *directly* — it never touches
`crate::render::budget::probe_and_select` (which is only called inside
`build_app_with_budget`, `lib.rs:168`). So the legacy e2e path skips the probe
purely by not routing through `build_app_with_budget`; the canonical budget
then comes from `build_app_core`'s defensive `EffectiveWorldSize::canonical()`
/ `InvalidSampleStorageCount::canonical()` seeds (`lib.rs:353,361`).

**Mirrored mechanism for `--e2e-brp`:** `main.rs`'s `--e2e-brp` branch boots
via `bevy_naadf::bootstrap::build_app_with_bootstrap_inputs(cfg, inputs)` —
NOT `build_app_with_budget`. `build_app_with_bootstrap_inputs` calls
`build_app_core` and fans out the `BootstrapInputs` (so `--vox` still installs
its world), but it never calls `probe_and_select`. The `BootstrapInputs` is
constructed `{ grid_preset, ..Default::default() }`, so `taa_ring_depth` is
the canonical `TaaRingConfig::default()` (= `DEFAULT_TAA_RING_DEPTH` = 32).
Net: the `--e2e-brp` boot path uses the canonical world / TAA / invalid-sample
rungs, exactly as `e2e_render` does today. The design §5's prose ("still
`build_app_with_budget`") is superseded by the brief's hard-gate resolution —
this implementation follows the brief.

This was confirmed at runtime: the optional sanity check (below) showed the
SUT's `prepare_world_gpu` allocating the canonical
`chunks=2097152 / blocks=512 MiB / voxels=1024 MiB` buffers — the canonical
256×32×256-chunk world, not a mobile rung.

### Gate results

All three gates from the brief, exact outcomes:

1. **`cargo build --workspace` (default features, no `e2e-brp`)** — PASS.
   `Finished dev profile in 1m 00s`, 0 errors, 0 warnings. The default
   production build compiles; all BRP code is behind `#[cfg(feature =
   "e2e-brp")]`.

2. **`cargo build -p bevy-naadf --features e2e-brp`** — PASS.
   `Finished dev profile in 44.29s`, 0 errors, 0 warnings. The BRP server
   scaffold + the three verbs compile against `bevy_remote 0.19.0-rc.1`.

3. **`timeout 180s cargo run --bin e2e_render -- --vox-e2e`** — PASS,
   **exit status 0**, well within the 180s budget. Output tail:
   ```
   e2e_render --vox-e2e: vox_geometry channel max (max of mean_R / G / B) = 251.8 (threshold > 30 ...)
   e2e_render: PASS (batch 6) — 96 warmup + 48 camera-motion + 1 settle frames,
     framebuffer read back & non-degenerate, per-batch region gate green through
     camera motion, every pipeline created cleanly, every expected render-graph
     node dispatched.
   ```
   The legacy in-app e2e path is unbroken — Phase 1 did not regress it.

**Optional sanity check (performed).** Booted the `e2e-brp` build
`target/debug/bevy-naadf --e2e-brp 15799` and curled the three verbs over
loopback HTTP:
- `naadf/get_state` → `{"frame":4573,"frames_remaining":0,"pipeline_errors":null,"tracing_errors":0,"world_loaded":true}`
- `naadf/step {frames:30}` → `{"frame":4574}`
- `naadf/run_until_idle {max_frames:200,idle_frames:5}` → streamed exactly one
  final chunk `{"done":true,"frame":4608,"timed_out":false}` (`4574 + 30`
  queued + the idle window ⇒ settled at 4608, not budget-hit — `timed_out:false`).
- `naadf/step` with `params:{}` → JSON-RPC `error {"code":-32602,"message":"naadf/step requires an integer \`frames\` field"}`.
All four behaved exactly as designed. (Note: the SUT was run from
`crates/bevy_naadf/` with the binary at `target/debug/`, so the
Phase-0-documented asset-path CWD warning appeared — irrelevant to the
transport test, which is what this sanity check covers.)

### Anything Phase 2 must know

- **The verb-builder chain holds.** `RemotePlugin::default()
  .with_method_main(...).with_watching_method_main(...)` chains cleanly;
  Phase 2 adds the remaining 8 verbs to the same chain in `install_brp_server`.
  The render-world verb `naadf/pipeline_scan` uses `with_method_render` — the
  render sub-app exists at the `build_app_core`-tail install point (verified:
  `DefaultPlugins` incl. `RenderPlugin` is added earlier in `build_app_core`).

- **`get_state.pipeline_errors` is a Phase 2 wiring point.** It is `null` in
  Phase 1 by design. Phase 2 (per design §6.3) moves the `PipelineScanResult`
  `Arc<Mutex>` channel + the render-world scan system into
  `install_brp_server`'s setup; once that lands, `get_state` will surface real
  pipeline health. The `get_state` handler already reads the channel
  Option-tolerantly (`get_resource`), so Phase 2 only needs to *insert* the
  resource — no `get_state` change required.

- **`run_until_idle` is single-watch.** `RunUntilIdleWatch` is one slot. If
  Phase 2+ ever needs concurrent `run_until_idle` watches (it should not — the
  runner is synchronous), the slot must become a per-request map. Flagged in
  the verb's doc comment.

- **`E2eControl` / `RunUntilIdleWatch` / `advance_e2e_control` live in
  `e2e_brp::verbs`** and are `pub` — Phase 2's `e2e_brp::schema` module and
  the `naadf_e2e` runner crate can reference them if needed (though the design
  keeps the wire schema as plain serde structs, separate from these
  resources).

- **Design §5 vs the brief on the budget probe.** Design §5 prose says the
  `--e2e-brp` path is "still `build_app_with_budget`"; the brief's binding
  hard-gate resolution says the SUT forces canonical budget and must NOT route
  through `probe_and_select`. This implementation follows the brief — see the
  `e2e_sut` budget handling section. Phase 2+ agents reading design §5 should
  treat that one sentence as superseded.

## Side notes / observations / complaints

- **The Phase 0 seed was a genuinely good seed.** `install_brp_server`'s
  shape, the install point, the `WinitSettings::Continuous` placement, the
  feature gating — all carried into Phase 1 unchanged in intent; Phase 1 only
  *filled in* the verb set and swapped the env-var opt-in for the
  `AppConfig::brp_port` field. No rework, no fighting the seed. The Phase 0
  recommendation ("keep the edits as the Phase 1 seed") was correct.

- **The `--e2e-brp` flag behaviour with the feature OFF is a deliberate, mild
  oddity worth naming.** With `e2e-brp` off, `cargo run --bin bevy-naadf --
  --e2e-brp 15799` parses the flag, selects `AppConfig::e2e_sut(15799)`, and
  boots the **e2e determinism profile with no BRP socket** (the `brp_port`
  field is read by no compiled code). It does not error and does not behave
  like `windowed()`. This is intentional and documented in `main.rs` — the
  flag failing cleanly on a typo'd port is worth more than silent
  feature-gated divergence, and the runner always builds the SUT `--features
  e2e-brp` so the real path is unaffected. But it is a (small) behavioural
  fork the orchestrator should be aware of: a developer who runs `--e2e-brp`
  on a default build gets a windowed app in the e2e profile (256×256, HUD off,
  no fly camera) and may be briefly confused. Not a defect — flagging it as a
  conscious tradeoff.

- **`build_app_with_budget`'s doc comment is now slightly stale.** It says
  "Production callers: Desktop + WebGPU/wasm32 — `src/main.rs::fn main()`" —
  still true for the non-`--e2e-brp` path, but `main.rs` now has a *second*
  native boot path (`--e2e-brp` → `build_app_with_bootstrap_inputs`) that
  deliberately bypasses it. I did not edit that doc comment (it is not wrong,
  just incomplete, and the `e2e_sut` doc comment + this log cover the new
  path). A future `/refactor` pass could tidy it; not worth a Phase 1 edit.

- **No foundation smell.** The `AppConfig` "deliberate deltas" carrier took a
  fifth field idiomatically; the boot funnel (`build_app_core` /
  `build_app_with_bootstrap_inputs`) had the exact seam the design named; the
  `bevy_remote` API matched the design's §0 first-hand reading. The
  restructure is not fighting the codebase. The only thing I'd push the
  orchestrator on is the design-§5-vs-brief contradiction above — it is
  resolved correctly here, but it is the kind of latent inconsistency that
  would have produced the wrong implementation if an agent followed design §5
  literally without the brief's hard-gate resolution. The brief caught it;
  the design body should be amended (in THIS orchestration's docs) so Phase 2+
  does not re-trip on it.

---

## Phase 2 — full verb set + runner crate + first gate (2026-05-22)

**Verdict: Phase 2 lands clean. All 5 verification gates green. The 8 new verbs
+ the `e2e_brp::schema` module + the pipeline-scan wiring landed in
`bevy_naadf`; the `naadf_e2e` runner crate is a new `lib`-only workspace
member; the `oasis_edit_visual` gate is migrated to
`crates/bevy_naadf/tests/oasis_edit_visual.rs` and passes on BOTH the new
BRP-driven path and the legacy `e2e_render` path with a sub-0.1 delta
divergence (Δ 17.96 vs 18.07 — TAA/GI shimmer-level). The default-feature
production build is unchanged; the legacy path is unbroken.**

One design correction (`naadf/pipeline_scan` is main-world, not render-world)
and the hybrid layout from the brief, both detailed below.

### What changed

**`bevy_naadf` (the SUT side):**

- `crates/bevy_naadf/src/lib.rs` — `pub mod e2e_brp;` is now declared
  **unconditionally** (was `#[cfg(feature = "e2e-brp")]`). Required so the
  `schema` sub-module compiles into every build (design D8 / A7) — the runner
  crate imports the verb wire structs without building `bevy_naadf` with
  `e2e-brp`. The handlers + `install_brp_server` stay feature-gated *inside*
  `e2e_brp/mod.rs`.
- `crates/bevy_naadf/src/e2e_brp/mod.rs` — `schema` declared unconditional;
  `verbs` + a new `mod install` (holding `install_brp_server`) gated
  `#[cfg(feature = "e2e-brp")]`. `install_brp_server` rewritten: chains all 11
  verbs, inserts the `PipelineScanResult` cross-world channel into both worlds,
  wires `scan_pipeline_errors_render_system` into the `RenderApp`, inits the
  `AwaitCaptureWatch` + `E2eScreenshot` resources (design §6.3 pipeline-scan +
  capture wiring).
- `crates/bevy_naadf/src/e2e_brp/schema.rs` (NEW, **unconditional**) — the
  plain-`serde` param/return structs for all 11 verbs (D8 / A7). No
  `bevy_remote` dependency.
- `crates/bevy_naadf/src/e2e_brp/verbs.rs` — the 8 new verb handlers added to
  the 3 Phase-1 verbs; plus the `AwaitCaptureWatch` + `LastCapture` support
  resources and the `encode_png_bytes` helper.
- `crates/bevy_naadf/Cargo.toml` — added `base64 = "=0.22.1"` (the
  `naadf/await_capture` PNG payload encoder; pinned to the version already in
  `Cargo.lock` transitively) and a `[dev-dependencies]` arrow to `naadf_e2e`
  (the gate test files in `tests/` use the runner).

**`naadf_e2e` (the runner side — NEW workspace member):**

- `Cargo.toml` (root) — `members` gains `crates/naadf_e2e`.
- `crates/naadf_e2e/Cargo.toml` (NEW) — `lib`-only crate, deps `serde`/
  `serde_json`/`base64`/`image` + `bevy-naadf` (`default-features = false`,
  for `e2e_brp::schema` + the pure `e2e::framebuffer` code). **No HTTP-client
  dependency** — see `BrpClient` below.
- `crates/naadf_e2e/src/lib.rs` (NEW) — crate root, re-exports `Sut`/`SutOpts`/
  `BrpClient` + `bevy_naadf::e2e_brp::schema`.
- `crates/naadf_e2e/src/client.rs` (NEW) — `BrpClient`.
- `crates/naadf_e2e/src/sut.rs` (NEW) — `Sut` / `SutOpts`.
- `crates/naadf_e2e/src/scenario.rs` (NEW) — the scenario helper layer.

**The migrated gate:**

- `crates/bevy_naadf/tests/oasis_edit_visual.rs` (NEW) — the `oasis_edit_visual`
  gate as a BRP-driven `#[test]`. Same-package as the `bevy-naadf` binary (the
  hybrid-layout decision).

### The 8 verbs

All wrap an existing primitive verbatim; each verified against current code
before wrapping.

- **`naadf/capture`** (instant, main) — wraps `e2e::readback::shoot_primary_window`
  (`readback.rs:34`). Clears the `E2eScreenshot` stash, then spawns the
  `Screenshot::primary_window()` entity via a one-shot `CommandQueue` applied
  inside the exclusive handler. Surprise: capture is genuinely async — the
  `ScreenshotCaptured` observer can take many *native* SUT frames to fire under
  post-edit GPU load (see `await_capture` below).
- **`naadf/await_capture`** (watching, main) — polls `E2eScreenshot`, decodes
  via `Framebuffer::from_image` (`framebuffer.rs:152`), encodes an in-memory PNG
  (`encode_png_bytes`, the in-memory twin of `Framebuffer::save_png`), base64s
  it, streams one chunk. Also stashes the decoded `Framebuffer` in a new
  `LastCapture` resource so `region_gate` can read it. **Surprise / tuning:**
  the first run used a 64-native-frame ceiling (mirroring the legacy
  `OASIS_DRAIN_FRAMES = 16`); the post-edit capture timed out because the SUT
  ticks at hundreds of FPS and the screenshot observer fires many native frames
  later when the renderer is under W2/W3 post-edit load. Raised the default
  ceiling to 2000 native frames (still sub-10 s wall-time, still a real
  fail-fast). This is a legitimate native-vs-driver-frame difference, not a
  fidelity compromise — the legacy 16 was *driver* frames at a controlled pace.
- **`naadf/apply_brush`** (instant, main) — wraps `editor::tools::{sphere_brush,
  cube_brush, paint_brush}` (`tools.rs:226-285`) directly on `ResMut<WorldData>`,
  ignoring `EditorState` (design D6). Returns `voxels_delta`/`blocks_delta`/
  `batches`. No surprise — the brush fns are pure and the deltas match the
  legacy `apply_erase_brush` log exactly (`voxels_delta 6528` on both paths).
- **`naadf/set_camera`** (instant, main) — mutates the `Camera3d` entity's
  `Transform` + `PositionSplit` (the same pair `pin_oasis_camera` writes,
  `oasis_edit_visual.rs:326-328`). `PositionSplit::from_world` (`position_split.rs:33`).
  No surprise.
- **`naadf/load_world`** (instant, main) — sets `Res<GridPreset>` (design §3
  table). **Demoted** per design §3.1 — `GridPreset` is consumed at `Startup`,
  so this verb cannot retroactively re-install the world; the 13 gates load
  their fixture through the `--vox` spawn flag instead. The verb is kept
  schema-complete (sets the resource) but is *not* on any gate's critical path.
- **`naadf/region_gate`** (instant, main) — wraps `Framebuffer::region_mean` +
  `Framebuffer::luminance` (`framebuffer.rs:237,277`) over the `LastCapture`
  framebuffer. No surprise.
- **`naadf/resize_window`** (instant, main) — `WindowResolution::set(f32,f32)`
  on the `PrimaryWindow` (design D10, replaces the `hyprctl` path). No surprise;
  not exercised by `oasis_edit_visual` (it is the `resize-test` gate's verb,
  Phase 3).
- **`naadf/pipeline_scan`** — **DESIGN CORRECTION: main-world, not render-world.**
  Design §3 / D7 specified `with_method_render` "because `PipelineCache` is a
  render-world resource." But the verb does not read `PipelineCache` — it reads
  the `PipelineScanResult` `Arc<Mutex>` *cross-world channel* (which D7 KEEPS),
  whose main-world clone carries the identical scan result the render-world
  `scan_pipeline_errors_render_system` writes. A render-world verb buys nothing
  here AND would force the runner onto `bevy_remote`'s render-subapp HTTP port
  (`RemoteHttpPlugin::render_port`, default `15703`, **no builder to override
  it** — `bevy_remote 0.19.0-rc.1` `http.rs:118`), which collides between
  concurrent gate processes. So `pipeline_scan` is `with_method_main`; the
  render-world scan *system* still runs in the render world (it must — it reads
  `PipelineCache` directly). This was caught at runtime — the first gate run
  with the render-world verb returned `-32601 Method not found` because the
  client only talks to the main port.

### The `naadf_e2e` crate

- **`BrpClient` — raw `TcpStream`, no HTTP-client crate, no SSE parser.** The
  design (A6) flagged `ureq` as the first choice with "raw `TcpStream` + manual
  HTTP/1.1" as the documented fallback. I took the fallback deliberately, and
  it is the *better* call here: (1) the transport is loopback HTTP on
  `127.0.0.1` — no TLS, no redirects, a general HTTP client is pure overhead;
  (2) **the watching verbs do not need client-side SSE.** `bevy_remote`'s HTTP
  layer only switches a response to `text/event-stream` when the *method name
  contains `+watch`* (`http.rs:386`). The `naadf/*` watching verbs are
  registered under their **bare names** (`naadf/run_until_idle`,
  `naadf/await_capture`), so the HTTP layer takes the `Complete` path — it does
  one `result_receiver.recv().await` and replies with a single
  `application/json` body. The watching *handler* still re-runs every SUT frame
  and streams `Ok(None)` until its single final `Ok(Some(..))`; that final
  value is exactly what the server's lone `recv()` delivers. Net: every
  `naadf/*` verb — instant or watching — is one blocking request / one JSON
  response from the client. `BrpClient` opens a fresh `Connection: close` TCP
  socket per call, reads to EOF, parses the JSON-RPC envelope. (It *also*
  carries a defensive chunked-transfer + SSE-last-frame decoder in
  `split_http_body` in case a future verb is registered with `+watch` — unused
  on the current verb set but cheap insurance.) `ureq` was never added — the
  runner's dep tree is `serde`/`serde_json`/`base64`/`image` only, no async
  runtime, no `hyper` tail.
- **`Sut` — process harness.** `Sut::spawn(SutOpts)` launches `bevy-naadf
  --e2e-brp <port> [--vox ..] [--e2e-window ..]`, sets the child `current_dir`
  to the `bevy_naadf` crate root (the Phase 0 forward-note — `AssetPlugin`'s
  `src/assets` shaders resolve relative to CWD), polls `rpc.discover` until the
  BRP server answers (bounded, default 60 s; panics if the child exits during
  boot), and `kill`+`wait`s the child on `Drop` (no orphans). The OS-assigned
  free-port path binds `127.0.0.1:0`, reads the port, drops the listener.
- **`scenario` helpers** — `advance` (`step` + `run_until_idle`, with a
  timed-out guard), `get_state`, `capture` (`capture` + `await_capture` +
  base64 PNG decode → `Framebuffer::from_raw_rgba`), `set_camera`,
  `erase_sphere`, `region_gate`, `pipeline_scan`, `resize_window`. The *pure*
  assertion math stays in `bevy_naadf::e2e::framebuffer`; the helpers only
  orchestrate verbs.

### The migrated `oasis_edit_visual` gate

`crates/bevy_naadf/tests/oasis_edit_visual.rs` — the gate as a straight-line
`#[test]` body following design §7.3's worked example.

**Hybrid layout (the brief's user-resolved fork).** The gate test file lives in
`crates/bevy_naadf/tests/`, NOT in `naadf_e2e/tests/`. Being same-package as
the `bevy-naadf` binary, Cargo sets `CARGO_BIN_EXE_bevy-naadf` for the test
binary — the test passes `env!("CARGO_BIN_EXE_bevy-naadf")` to `Sut::spawn`,
which locates the SUT with **no `cargo build` shell-out** (design §7.2's
`OnceLock`-guarded build dance is the separate-crate path and is unnecessary
here). `SutOpts::cwd` is `env!("CARGO_MANIFEST_DIR")` = the crate root.

**Ported constants — verbatim** from `e2e/oasis_edit_visual.rs`:
`OASIS_WARMUP_FRAMES = 120`, `OASIS_POST_EDIT_WAIT_FRAMES = 300`,
`OASIS_ERASE_RADIUS = 30.0`, `OASIS_DIFF_RECT_FRACS = (0.35,0.35,0.65,0.65)`,
`OASIS_EDIT_DIFF_FLOOR = 8.0`, plus the `birdseye_pose` / `world_centre_voxel`
geometry. The assertion math (`region_mean_pixel_delta` — a private fn in the
legacy module) is ported verbatim into the test file; `Rect::from_fractional`
+ `Framebuffer::region_mean` / `mean_pixel_delta` are reused from `bevy_naadf`
unchanged.

**Dual-path result — the fidelity proof:**

| | rect | rect mean per-pixel RGB Δ | floor | verdict |
|---|---|---|---|---|
| new BRP path | `(89,89,166,166)` | **17.96** | 8.00 | PASS |
| legacy `e2e_render` | `(89,89,166,166)` | **18.07** | 8.00 | PASS |

Same gate, same rect, same `voxels_delta 6528` brush footprint, Δ divergence
0.11 — TAA/GI shimmer-level noise. The migration reproduces the gate faithfully.

### Gate results — all 5 green

1. **`cargo build --workspace` (default features)** — PASS, `Finished dev
   profile in 1m 08s`, 0 errors. The production binary is unchanged (all BRP
   handler code stays behind `#[cfg(feature = "e2e-brp")]`; only the
   handler-free `schema` module is newly always-compiled).
2. **`cargo build -p bevy-naadf --features e2e-brp`** — PASS, `Finished dev
   profile in 1m 05s`, 0 errors. (Note: the package name is `bevy-naadf` with a
   hyphen — `-p bevy_naadf` is rejected with "packages outside of workspace".)
3. **`cargo build -p naadf_e2e`** — PASS, `Finished dev profile in 28.05s`,
   0 errors, 0 warnings.
4. **`cargo test -p bevy-naadf --features e2e-brp --test oasis_edit_visual`** —
   **PASS**, `test result: ok. 1 passed`, finished in 8.03 s (well inside the
   ~300 s timeout). Output tail:
   ```
   oasis-edit-visual: erase sphere centre [2048.0, 256.0, 2048.0] r=30 — voxels_delta 6528 blocks_delta 0 batches 2
   oasis-edit-visual: rect=(89,89,166,166) ... rect mean per-pixel RGB Δ=17.96 (floor 8.00); full-frame Δ=4.37
   oasis-edit-visual: PASS — rect mean per-pixel RGB Δ 17.96 >= floor 8.00
   ```
   PNGs on disk: `crates/bevy_naadf/target/e2e-screenshots/oasis_edit_before.png`
   (118699 B) + `oasis_edit_after.png` (118893 B) — the SUT's CWD is the crate
   root so the runner-saved PNGs land under `crates/bevy_naadf/target/`.
5. **`cargo run --bin e2e_render -- --oasis-edit-visual`** (legacy path) —
   **PASS, exit status 0**, well inside the 240 s timeout. Output tail:
   ```
   e2e_render --oasis-edit-visual: rect=(89,89,166,166) ... rect mean per-pixel RGB Δ=18.07 (floor=8.00) ...
   e2e_render: oasis-edit-visual PASS — 120 warmup + 300 post-edit wait frames; erase sphere @ r=30.0 voxels produced rect mean per-pixel RGB Δ above 8.00 floor.
   ```
   Legacy PNGs: `target/e2e-screenshots/oasis_edit_{before,after}.png`. The
   legacy harness is unbroken — Phase 2 did not regress it.

### Anything Phase 3 must know

- **The migrated-gate pattern Phase 3 replicates.** Each gate becomes a
  `crates/bevy_naadf/tests/<gate>.rs` `#[test]` body: `Sut::spawn(SutOpts::new(
  env!("CARGO_BIN_EXE_bevy-naadf"), env!("CARGO_MANIFEST_DIR")).vox(..).window(..))`
  → `scenario::get_state` for world size → `scenario::set_camera` → `advance` /
  `capture` / `erase_sphere` / `region_gate` / `pipeline_scan` → assert with
  constants ported verbatim from the gate's `e2e/<gate>.rs` module +
  `bevy_naadf::e2e::framebuffer` for the pure math. `oasis_edit_visual.rs` is
  the template.
- **Package name.** It is `bevy-naadf` (hyphen) — `cargo test -p bevy-naadf
  --features e2e-brp --test <gate>`. The brief's `-p bevy_naadf` form is
  rejected by Cargo. The library crate is `bevy_naadf` (underscore) — that is
  the `use` path, unaffected.
- **`naadf/pipeline_scan` is main-world** (design correction above) — Phase 3
  gates call it like any other verb on the one SUT port. Do NOT reintroduce a
  `with_method_render` registration.
- **`await_capture` native-frame ceiling.** A gate that captures after heavy
  GPU work should pass an explicit `max_frames` to `naadf/await_capture` if
  2000 native frames is somehow too tight (the `scenario::capture` helper
  passes 2000). The legacy `*_DRAIN_FRAMES` constants are *driver* frames and
  do NOT port 1:1 — they were a controlled-pace count; the BRP SUT ticks
  free-running.
- **The 2 "compare" gates** (`vox-gpu-oracle`, `vox-web-parity`) drive the SUT
  twice (or two SUTs) and call `bevy_naadf::e2e::ssim` on the two captures —
  `Framebuffer` decode is already exposed via `scenario::decode_png_b64` /
  `capture`. `resize-test` uses `scenario::resize_window` (the `hyprctl`
  dependency is gone). `--entities` is boot-time config — it likely needs a
  small `--e2e-entities` spawn flag (design side-note); Phase 3 sizes that.
- **No `naadf_e2e/tests/`** — the runner crate is `lib`-only by the hybrid
  decision. All 13 gate files live in `crates/bevy_naadf/tests/`.

## Side notes / observations / complaints

- **The design's render-world `naadf/pipeline_scan` (D7) was wrong, and the
  reason is structural, not a nitpick.** `bevy_remote`'s `RemoteHttpPlugin`
  serves the render sub-app on a *separate, second port* (`render_port`,
  default `15703`) and exposes **no builder to set it** — only `with_port` for
  the main port. So any render-world verb (a) lives on a different socket the
  client must separately target, and (b) is pinned to a fixed `15703` that
  collides the moment two gate processes run concurrently (which `cargo test`'s
  one-process-per-`tests/`-file model does by default — A5). The design's own
  D7 already KEEPS the `PipelineScanResult` cross-world channel, and
  `get_state` already reads its main-world clone — so the render-world verb was
  reading the exact same `Arc<Mutex>` data from the worse end. Moving it
  main-world is strictly better. Phase 3+ should treat "render-world BRP verb"
  as a smell unless something genuinely *only* exists in the render world and
  has no cross-world channel — and even then, weigh the fixed-port cost.
- **The watching-verb-as-blocking-call discovery simplified the runner a lot.**
  The design (A6, §7.1) sized the SSE handling as "the fiddly part" and the
  Phase 1 log described `run_until_idle` as having "streamed" a chunk. In fact,
  because the `naadf/*` watching verbs are registered under bare names (no
  `+watch` suffix), `bevy_remote`'s HTTP layer never puts them on the SSE path
  — it does a single blocking `recv()` and returns one `application/json` body.
  The watching *handler* semantics (re-run every frame, emit one final
  `Ok(Some)`) are unchanged and exactly what we want; the *client* just sees a
  normal blocking request. So `naadf_e2e` needs no SSE parser and no HTTP
  client at all — raw `TcpStream` is ~250 lines and zero deps. If a future verb
  genuinely needs incremental streaming it must be registered *with* `+watch`
  in the name; the `BrpClient` already has a defensive SSE/chunked decoder for
  that day.
- **`await_capture`'s frame ceiling is the one place a legacy constant did NOT
  port 1:1.** Every other ported constant (`OASIS_*`) is a verbatim copy. But
  `OASIS_DRAIN_FRAMES = 16` was a *driver-frame* count at the legacy harness's
  controlled pace; the BRP SUT free-runs at hundreds of FPS, so the equivalent
  native-frame ceiling is ~100× larger. This is a real semantic difference
  between the two harnesses, not a fidelity compromise — flagging it because a
  Phase 3 agent porting a capture-heavy gate could mis-size it by copying a
  `*_DRAIN_FRAMES` literal. The *assertion* thresholds (`OASIS_EDIT_DIFF_FLOOR`
  etc.) port verbatim and must continue to.
- **`AppConfig` package-name vs crate-name friction.** The brief and design say
  `cargo test -p bevy_naadf`; the package is `bevy-naadf` (hyphen) and Cargo
  rejects the underscore form outright ("packages outside of workspace"). The
  library crate is `bevy_naadf`. Not a defect — just a doc/reality mismatch the
  Phase 3 brief should state correctly so an agent does not lose time on it.
- **No foundation smell in the verb-wrapping work.** Every primitive the verbs
  wrap (`shoot_primary_window`, the brush fns, `Framebuffer`, the
  `PipelineScanResult` channel) was exactly as clean and reusable as the design
  §-Side-notes claimed — each verb is a genuinely thin wrapper. The only design
  defect found is the render-world `pipeline_scan` call, corrected above. The
  restructure is not fighting the codebase.
- **Schema-as-unconditional-module (D8 / A7) worked exactly as designed.** The
  `e2e_brp::schema` module compiles into the default `cargo build --workspace`
  (it is just `serde` structs); `naadf_e2e` imports it via a plain
  `default-features = false` dep on `bevy-naadf`; no third crate, no feature
  scramble. The one edit it cost was making `pub mod e2e_brp;` unconditional in
  `lib.rs` and gating `verbs` + `install` *inside* the module — clean.
