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
