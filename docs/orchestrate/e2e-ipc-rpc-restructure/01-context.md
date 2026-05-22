# 01 — Context bundle: e2e harness restructure to BRP-controlled production app

> Canonical context for every non-review agent in the `e2e-ipc-rpc-restructure`
> orchestration. Read this file in full before doing anything else.
>
> All work happens in the git worktree
> `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build`, branch
> `feat/android-build`. Every relative path below is from that worktree root.

---

## Behavioural problemspace

**How it behaves now.** The bevy-naadf e2e harness is *baked into the app* as
in-app "driver modes." A separate binary `bin/e2e_render` carries a hand-rolled
3-layer argv parser (~22 flags); booting a gate installs an `e2e_driver` state
machine (`match`-over-`E2ePhase`, ~26 phase variants) that advances a fixed
frame budget, fires brushes/camera-pins, captures framebuffers, runs assertions,
and writes `AppExit`. The `E2eGateMode` enum resource routes which gate runs.
The production binary `bin/bevy-naadf` and `bin/e2e_render` are *structurally
the same App* — they diverge only in `AppConfig` (`windowed()` vs `e2e()`); the
e2e variant flips on `add_e2e_systems`, `synchronous_pipeline_compilation`, a
fixed window size, continuous winit, and `add_hud=false`.

**How it is supposed to behave.** The *production* app `bin/bevy-naadf` is the
system-under-test. An external test runner spawns it in another process and
drives it through a functional remote interface. Each e2e scenario becomes an
ordinary test-case body that issues control calls — e2e is no longer "another
in-app mode" but the real app under external control.

**What the user wants.** A remote control surface exposing enough functional
verbs to automate any/all e2e scenarios as test-case bodies, so e2e modes are
not in-app modes. **The control layer is decided: Bevy's first-party Bevy
Remote Protocol (BRP, the `bevy_remote` crate).** The user picked it after a
survey established BRP ships *inside* the project's exact Bevy version. Success
criterion: the 13 booted-window e2e gates run as BRP-driven test-case bodies
against the spawned production binary, with their determinism and assertion
fidelity preserved.

---

## Restated goal — user verbatim

From the originating handoff (`/tmp/e2e-ipc-rpc-restructure-handoff.md`):

> "what about restructuring e2e modes as app-control modes - test runner spans
> up app in another process and utilises ipc to control it? in such case, if
> the method prooves viable, lets make this as a followup"

> "rpc over ipc with functional interface exposing enough surface to automate
> any and all e2e scenarios as bodies of test cases, so that e2e modes are not
> implemented as another in-app mode, but real app that is controlled via ipc
> rpc"

After the control-layer survey, the user chose BRP verbatim:

> "bevy remote is a wonderful choice. lets proceed with BRP"

---

## Decisions (binding — from the orchestration Q&A and survey)

1. **Adopt BRP.** The control layer is Bevy's first-party Bevy Remote Protocol
   (`bevy_remote`), which ships inside the project's pinned `bevy = "=0.19.0-rc.1"`.
   The work is NOT a from-scratch custom RPC stack — BRP already provides the
   JSON-RPC 2.0 framing, request dispatcher, method registry (`RemoteMethods`),
   per-frame mailbox drain, the `In(Option<Value>)` + `&mut World` handler
   model, the watching/streaming model, and a working HTTP transport
   (`RemoteHttpPlugin`). The project's *domain* verbs are custom BRP methods
   registered via `with_method_main` / `with_method_render` (0.19 splits
   registration by world). Source: user decision + `00-control-layer-survey.md`.

2. **Scope = the 13 booted-window gates.** `bin/e2e_render` dispatches ~22
   entries, but only 13 are genuine booted-window in-app driver modes. The
   other 9 are already-headless `pub fn → Result`/`u8` validators
   (`validate_*` + `ssim_compare_command` + the two `compare` orchestrators) —
   they are NOT in-app driver modes and are **out of scope** for this
   restructure. Relocating them to `tests/` is an independent trivial item
   (the audit's "Option C"); this orchestration does not own it. Source:
   orchestrator decision, grounded in `00-reuse-audit.md` § E2e-entry
   enumeration + Side notes.

3. **Native-only.** `RemoteHttpPlugin` is native-only; there is no first-party
   wasm BRP transport, and no Android e2e exists today. This restructure is
   scoped to the **native** e2e gates. wasm/Android remote control is a
   separate, later problem. Source: `00-control-layer-survey.md` § 3.6 + § 7.

4. **Distributed mode, design phase first.** This orchestration runs the
   *design* phase to completion (`02-design.md`), then stops at a hard gate
   where the user reviews the design. Whether implementation runs in this
   orchestration or spins out is decided at that gate. Implementation does NOT
   begin without the user's review of the design.

---

## Reuse audit summary (relayed from `00-reuse-audit.md`)

All `file:line` below were verified by the auditor at HEAD `ee87400`; re-verify
with Read/Grep before relying on any of them.

| Candidate | Location | Verdict |
|---|---|---|
| `bootstrap::BootstrapInputs` carrier + `build_app_with_bootstrap_inputs` fan-out | `crates/bevy_naadf/src/bootstrap.rs:48-225` | **extend** — the single boot funnel; the natural place a BRP server plugin conditionally installs |
| `e2e/readback.rs` `E2eScreenshot` + `shoot_primary_window`; `framebuffer.rs` `Framebuffer` | `readback.rs:14-37`, `framebuffer.rs:137-518` | **reuse** — transport-agnostic capture + PNG; becomes the `capture_framebuffer` verb implementation |
| `e2e/ssim.rs` `ssim_compare_command` | `crates/bevy_naadf/src/e2e/ssim.rs` | **reuse** — pure CPU PNG diff; a test body calls it on two captures |
| `e2e/checks.rs` `PipelineScanResult` cross-world `Arc<Mutex>` channel | `e2e/checks.rs:37-44`, inserted at `e2e/mod.rs:230,246,311` | **extend** — the project's only cross-world data-channel precedent |
| `e2e/gate.rs` `E2eGateMode` enum resource | `e2e/gate.rs:48-88` | **extend** — see open question below; may become near-vestigial |
| `editor/` brushes (`paint_brush`/`cube_brush`/`sphere_brush`) + `EditorState` | `editor/mod.rs:44-225`, `editor/tools.rs:226-277` | **extend/reuse** — the brush fns are pure `(&mut WorldData, …)` calls, directly callable from a BRP method |
| `e2e/driver.rs` `e2e_driver` state machine | `driver.rs:61,254,263,452+` (~1400 lines) | **not applicable as-is** — IS the in-app-driver-mode pattern being retired; its phase steps are the *spec* for the new verbs, see open question |
| `bin/e2e_render.rs` 3-layer argv parser | `crates/bevy_naadf/src/bin/e2e_render.rs` (~547 lines) | **not applicable** — the explicit deletion target |

**Production app boot path (the would-be SUT):** `bin/bevy-naadf` → `main.rs:34`;
argv is only `--vox <path>`. Native calls `build_app_with_budget(AppConfig::windowed(),
grid_preset)` (`lib.rs:161`) then `.run()`. `build_app_core` (`lib.rs:191`) is
the single plugin-pyramid funnel; `build_app_with_bootstrap_inputs`
(`bootstrap.rs:148`) is the fan-out wrapper every e2e gate uses.

**Workspace:** single-crate workspace (`members = ["crates/bevy_naadf"]`);
`crates/bevy_naadf` is `lib + cdylib` with 3 binaries. No `tests/` directory.
A separate test-runner crate or BRP-schema crate means adding a new workspace
member.

---

## Control-layer survey summary (relayed from `00-control-layer-survey.md`)

- **Bevy version: `=0.19.0-rc.1`** (exact pin). `bevy_remote` / the `remote`
  cargo feature is OFF today; no IPC/RPC/socket code exists anywhere.
- **BRP ships inside `bevy 0.19.0-rc.1`.** The whole third-party ecosystem
  (`bevy_brp_extras`, `bevy_brp_mcp`, `bevy_mod_scripting`) lags at Bevy 0.18;
  BITT is stale at 0.13. BRP is the only control layer at the project's exact
  version, by construction.
- **BRP built-in verbs** operate on **reflected** types only — a resource is
  reachable by `world.get_resources`/`world.mutate_resources` iff it is
  `#[derive(Reflect)]` + `register_type`'d.
- **Custom-method hook (0.19-rc.1 API, world-split):**
  `RemotePlugin::default().with_method_main(name, handler)` /
  `.with_method_render(name, handler)` (+ `with_watching_method_*`). Handler is
  an ordinary Bevy system `fn(In(Option<Value>), &mut World) -> BrpResult` with
  full exclusive world access. The render-world variant matters: this project's
  framebuffer readback is render-world.
- **Transport:** `RemoteHttpPlugin` (HTTP, JSON-RPC 2.0, default port 15702)
  ships in-tree, native-only. The transport-agnostic seam is the `BrpSender`
  channel mailbox — a non-HTTP transport is a custom *plugin pushing into
  `BrpSender`*, not a custom protocol.
- **What stays custom under BRP:** (a) domain verbs as thin custom methods
  wrapping existing primitives; (b) frame-stepping semantics (BRP drains one
  request per frame — "run N frames then reply" is awkward); (c) the transport
  choice; (d) boot-time determinism config (see Forbidden moves #4).

---

## Required reading (in order)

1. `docs/orchestrate/e2e-ipc-rpc-restructure/01-context.md` — this file.
2. `docs/orchestrate/e2e-ipc-rpc-restructure/02-design.md` — the design group
   file (your deliverable lands here).
3. `docs/orchestrate/e2e-ipc-rpc-restructure/00-reuse-audit.md` — full reuse
   audit + the auditor's `## Borderline calls` and `## Side notes`. Working
   memory of THIS orchestration — load-bearing.
4. `docs/orchestrate/e2e-ipc-rpc-restructure/00-control-layer-survey.md` — the
   BRP survey: the options table, the BRP deep-dive (built-in verbs, the
   custom-method API verified at the `v0.19.0-rc.1` git tag, transports), the
   coverage matrix, and the survey's `## Side notes`. Working memory of THIS
   orchestration — load-bearing.
5. `docs/orchestrate/config-as-resource-refactor/03-e2e-as-tests-investigation.md`
   and `docs/orchestrate/config-as-resource-refactor/04-followup-ipc-rpc-direction.md`
   — prior investigation of this exact question, written by sub-agents during a
   **different** orchestration. These are **journals, not canon** — story of
   what agents thought in the past, prone to hallucination. Read them for leads
   only; **verify every load-bearing claim against current code (`file:line` at
   HEAD) before relying on it.** The audit already noted line-number drift in
   `03`'s § 1.2 — the doc's prose/structure is accurate, its line refs stale.
6. Repo files (verify line ranges with Read/Grep — the audit's numbers may have
   drifted):
   - `crates/bevy_naadf/Cargo.toml` — Bevy dependency + feature lists.
   - `crates/bevy_naadf/src/bootstrap.rs` (~48-225) — `BootstrapInputs` + the
     `build_app_with_bootstrap_inputs` fan-out.
   - `crates/bevy_naadf/src/lib.rs` (~136-225) — `build_app_core`, `build_app`,
     `AppConfig`, the production funnel.
   - `crates/bevy_naadf/src/main.rs` — production app entry / argv handling.
   - `crates/bevy_naadf/src/bin/e2e_render.rs` — the 3-layer parser (deletion
     target).
   - `crates/bevy_naadf/src/e2e/` — `driver.rs`, `gate.rs`, `mod.rs`,
     `readback.rs`, `framebuffer.rs`, `checks.rs`, `ssim.rs`, and the 13
     per-gate files (`vox_e2e.rs`, `oasis_edit_visual.rs`,
     `small_edit_visual.rs`, `small_edit_repro.rs`, `vox_gpu_construction.rs`,
     `vox_gpu_oracle.rs`, `vox_web_parity.rs`, `vox_horizon_parity.rs`, …).
   - `crates/bevy_naadf/src/editor/mod.rs`, `editor/tools.rs` — brushes.
   - `e2e/` (the Playwright dir) — the cross-target parity spec that currently
     spawns `cargo run --bin e2e_render`.

---

## Open questions / unresolved forks (resolve from code + canon — NOT pre-decided)

Quoted from `00-reuse-audit.md` `## Borderline calls` and `00-control-layer-survey.md`
`## Side notes`. These are the architect's to navigate; none are settled.

- **`e2e_driver` state machine — discard the orchestration vs extract its
  primitives.** Auditor: *"The architect must decide whether RPC verbs are thin
  wrappers over driver-internal helpers (extract) or a fresh surface (discard
  the orchestration only)."* The `match`-over-`E2ePhase` orchestration is the
  in-app-driver-mode pattern being retired; the *primitives* it calls
  (frame-budget counting, `shoot_primary_window`, brush calls, `run_assertions`)
  are exactly the BRP verbs a runner needs.

- **`E2eGateMode` enum — extend vs becomes vestigial.** Auditor: *"if the
  restructure fully removes `add_e2e_systems` from the SUT, the driver and
  `pin_*_camera` systems leave with it, and `E2eGateMode`'s only remaining
  reader is `window_for_gate_mode` (window sizing) … The architect must resolve
  this against the 'no e2e modes baked into the app' principle."*

- **`editor/` brushes — extend `EditorState` vs reuse the brush fns directly.**
  Auditor: *"whether the RPC verb needs `EditorState`'s smoothed-`pos`/stroke
  semantics (extend the resource) or just one-shot brush application (reuse the
  fn, ignore the resource)."*

- **BRP transport choice.** `RemoteHttpPlugin` (HTTP over loopback, zero project
  code, native-only) vs a custom IPC transport plugin (Unix socket / stdio /
  pipe — a plugin pushing into `BrpSender`, not a custom protocol). The user
  declined to pre-decide and left this to the architect. Resolve it — and if a
  spike is warranted, do one.

- **Frame-stepping semantics over BRP.** BRP drains one request per frame;
  "advance exactly N frames deterministically, then let me assert" fights that
  model. Survey: *"a watching method that counts frames down, the runner
  issuing N calls, or a custom method that pumps the schedule"* — the architect
  designs this. It is the survey-flagged hard 80% of the work.

- **Do the config resources derive `Reflect`?** BRP's built-in resource verbs
  only see `#[derive(Reflect)]` + registered types. The audit confirms
  `E2eGateMode`/`GiSettings`/`GridPreset`/`ConstructionConfig`/`TaaConfig` are
  `#[derive(Resource)]` but is silent on `Reflect`. **Grep this** — it decides
  whether get/set-resource is free or needs a `Reflect` derive / custom getter
  per type.

- **`bevy_brp_extras`.** It ships ready-made `screenshot` + input-injection BRP
  methods, but is built for Bevy 0.18 and unconfirmed on 0.19-rc.1. Check
  whether a 0.19-compatible release exists; if not, do NOT hard-depend on it
  (the project already has `e2e/readback.rs` doing screenshot capture).

- **Cross-target Playwright gate.** `e2e/tests/vox-horizon-parity.spec.ts`
  spawns `cargo run --bin e2e_render` as a subprocess. If `bin/e2e_render` is
  deleted, that spec breaks. The architect must address what happens to this
  gate — surface the treatment in the design; do not silently strand it.

---

## Forbidden moves (solution constraints — hard provenance)

1. **Do NOT design a from-scratch custom RPC protocol or an Option-A/B/C
   custom-RPC framework.** The control layer is BRP — a user decision ("lets
   proceed with BRP"). Custom work is custom *BRP methods* and (if needed) a
   custom *BRP transport plugin* — never a parallel protocol. Provenance: user
   Q&A decision.

2. **The 9 already-headless `validate_*` / `compare` / `ssim_compare`
   entries are out of scope.** This restructure targets the 13 booted-window
   gates only. Do not design their migration. Provenance: orchestrator decision
   grounded in `00-reuse-audit.md` § E2e-entry enumeration.

3. **No wasm or Android e2e in this restructure.** `RemoteHttpPlugin` is
   native-only and no Android e2e exists today. Native-only. Provenance:
   `00-control-layer-survey.md` § 3.6 (code fact: the transport is gated
   `not(target_family = "wasm")`).

4. **Do NOT design a BRP verb that sets boot-time determinism knobs.**
   `synchronous_pipeline_compilation`, fixed window size, `WinitSettings::Continuous`
   and the GI warmup budget are set inside `BootstrapInputs` *before*
   `app.run()`; BRP exists only *after* the app is running. These must reach
   the SUT through the **spawn contract** (CLI flag / env var on the
   subprocess), not an RPC verb. Provenance: code fact — verified in
   `00-reuse-audit.md` § Production app boot path + `00-control-layer-survey.md`
   § 3.5/§ 7. (How the spawn contract is shaped IS the architect's design —
   only "an RPC verb cannot do it" is forbidden.)
