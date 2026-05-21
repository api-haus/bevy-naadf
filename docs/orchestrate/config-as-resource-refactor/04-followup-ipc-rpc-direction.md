# Follow-up direction — IPC-RPC controlled e2e

**Status:** captured-but-not-orchestrated. Surfaced by the user at the AppArgs-refactor design review (2026-05-21) as the long-term direction for the e2e harness. This document captures the direction so it isn't lost; the actual viability investigation + implementation is a **separate orchestration** the user will invoke when ready.

---

## The user's stated direction (verbatim)

> what about restructuring e2e modes as app-control modes - test runner spans up app in another process and utilises ipc to control it? in such case, if the method prooves viable, lets make this as a followup

> rpc over ipc with functional interface exposing enough surface to automate any and all e2e scenarios as bodies of test cases, so that e2e modes are not implemented as another in-app mode, but real app that is controlled via ipc rpc

## What this proposes (paraphrased — not a design)

The **production app** `bin/bevy-naadf` becomes the SUT. It exposes an RPC server (presumably via a Bevy plugin) that accepts a functional surface broad enough to drive any e2e scenario. Each `#[test]` body in `tests/<scenario>.rs`:

1. Spawns `bevy-naadf` as a subprocess.
2. Connects to its RPC endpoint.
3. Drives the scenario via RPC calls (load world, step frames, inject input, query state, capture framebuffer).
4. Asserts on responses.
5. Tears down the subprocess.

**No e2e modes baked into the app.** No `bin/e2e_render`. The app is unconditionally drivable by tests through the same RPC interface a developer could use for automation tooling.

## Why this is structurally cleaner than alternatives in `03-e2e-as-tests-investigation.md`

Option B (in-process `tests/<gate>.rs`) keeps the e2e harness inside the binary; the app still has test-only modes/state. Option C (only validators move) leaves the rotten `bin/e2e_render` intact. The IPC-RPC proposal sidesteps both:

- **No more "test-only modes baked into the app"** — the app has no notion of "e2e_mode". Scenario shape lives in the test body.
- **One RPC schema, many clients** — Rust integration tests, Playwright, ad-hoc automation tools, and a possible interactive console all consume the same surface.
- **WinitPlugin event-loop constraint dissolves** — each test spawns a fresh subprocess; no shared event loop, no `serial_test::serial(winit)` ceremony.
- **`App::run()` consuming the App is no longer a problem** — the app's `run()` continues normally; the test reads verdicts via RPC queries, not via post-run resource introspection.
- **Subprocess-orchestrator gates collapse** — `--vox-gpu-oracle` and `--vox-web-parity` become two RPC sessions inside one `#[test]` body that compares their outputs.
- **`bin/e2e_render` deletes** (or shrinks to a developer-facing SSIM utility), eliminating the 524-line rotten three-layer parser entirely.
- **The half-done `Gate` trait + `GateKind` enum scaffolding** at `e2e/gate.rs:30-127` is no longer load-bearing — the per-gate state machines move into test bodies as RPC call sequences.

## Open viability questions (the follow-up investigation must answer these)

1. **Bevy Remote Protocol (BRP) coverage.** Bevy 0.15+ ships an official BRP (JSON-RPC over HTTP, entity/component queries + mutations). Does BRP cover the surface the user envisions, or is a custom RPC layer needed on top? If BRP is sufficient, the follow-up is small; if a custom layer is needed, it's large.
2. **Frame-stepping under external control.** Can the production runner be set up to: tick → process RPC requests → tick → process RPC requests → ...? Or does the RPC happen concurrently (reads any-time, writes queued for the next system tick)? Determinism implications for frame-numbered assertions.
3. **Output capture surface.** Framebuffer screenshots (existing `e2e/checks.rs` cross-world readback is precedent), log lines, panic state, resource values. Wire format: PNG bytes for framebuffer; JSON for queries.
4. **Cross-target reach.**
   - **Native:** Unix socket / TCP loopback. Straightforward.
   - **wasm32:** browser context — IPC via `postMessage` requires a controller window; **Playwright already IS that controller**. Likely wasm32 stays on Playwright's existing approach and the IPC-RPC layer is native-only (Playwright drives wasm via CDP, IPC drives native via socket — both end up calling into the same RPC schema if it's transport-agnostic).
   - **Android:** TCP loopback if the test runner runs on the device, ADB-port-forwarded TCP if the test runner is on the host. The handoff's `docs/todo/android-build.md:86-107` deploy path is relevant here.
5. **Production-time impact.** Is the RPC server always-on, feature-gated, or cfg-gated? Likely feature-gated (`cargo run --features e2e-rpc`) — tests spawn the app with the feature; production builds omit it.
6. **Subprocess setup cost.** Each test spawns the app (~seconds for wgpu adapter init). N tests × M seconds = test-run total. Mitigation: per-test subprocess (clean state, slow) OR persistent subprocess reset between tests via RPC (fast, state leaks). Trade-off the follow-up has to pick.
7. **Existing precedent.** Bevy's official BRP is the closest analogue. Other game engines: Unreal's RemoteControl, Unity's via reflection, Godot's GDScript headless mode. Worth checking what BRP can do today before designing a custom layer.
8. **RPC API surface design.** Map each current gate to the RPC primitives that would replace it:
   - `--baseline` = "load default scene, step 96 frames, capture framebuffer, SSIM against fixture"
   - `--oasis-edit-visual` = "load default scene, step N frames, inject brush stroke at world coord, step M frames, capture, SSIM"
   - `--vox-gpu-construction` = "load .vox at path, step N frames, query construction result resource, compare against CPU oracle output"
   - …per-gate primitives accumulate into the schema.
9. **Test ergonomics.** What does the `#[test]` body actually look like? Helper functions in `tests/common/` to spawn-and-connect? Async runtime (tokio) or sync RPC? Error handling on subprocess crash mid-test? Subprocess timeout management.
10. **Comparing to in-process `tests/<gate>.rs` (`03-e2e-as-tests-investigation.md` Option B).** Pros: clean separation, real-app testing, language-agnostic schema, no winit serialization. Cons: subprocess cost, RPC schema design+maintenance, IPC failure-mode surface, additional layer between test intent and assertion.

## Relationship to the in-flight AppArgs refactor

The AppArgs refactor's `02-design.md` Step 6 (collapsing 11 e2e-mode booleans into `E2eGateMode` enum resource) is a **stepping stone** toward this direction, not an obstacle:

- Today: bootstrap-time CLI parser flips one of 11 booleans → driver branches on the boolean.
- After Step 6: bootstrap-time CLI parser sets `Res<E2eGateMode>` → driver branches on the enum.
- After IPC-RPC follow-up: test body issues `set_resource(E2eGateMode::Foo)` over RPC → driver branches on the enum, same way.

The enum resource is the natural RPC-controllable handle. **No work in `02-design.md` is wasted by the IPC-RPC direction.** Implementing the AppArgs refactor first leaves the codebase in a state where the IPC-RPC layer has a clean enum surface to flip rather than 11 booleans to coordinate.

## What this orchestration does NOT do

- **No design** of the RPC schema, transport, frame-stepping protocol, or test-harness shape.
- **No code** changes related to RPC, BRP, IPC, or subprocess management.
- **No viability assessment** beyond surfacing the dimensions that need investigation.

## When to invoke the follow-up orchestration

Likely sequence:

1. **Now → in-flight AppArgs refactor implementation** (per `02-design.md`, downstream orchestration).
2. **After AppArgs refactor lands → optionally Option C** from `03-e2e-as-tests-investigation.md` (7 validators to `tests/`, ~200 LOC, ~hours of work, ZERO blast radius on anything else). This is independent of the IPC-RPC direction.
3. **When ready → IPC-RPC viability orchestration**. Suggested entry: `/delegate` with a brief that references THIS document + asks for the viability investigation (BRP coverage, frame-stepping model, subprocess cost, per-gate RPC primitive map). If viable: a design orchestration; then implementation orchestration. Each is its own `/delegate` session.

The follow-up should NOT be folded back into the AppArgs refactor implementation. They are independent concerns with independent verification surfaces.
