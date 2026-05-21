# Control-layer survey — is custom RPC-over-IPC the right call?

> Read-only research dispatch. Question under study (verbatim from the user):
> *"is there a better way to control the app than custom rpc? there's gotta be
> bevy control-layers compatible with bevy .19 or at least .18 to look at"*
>
> Scope: survey Bevy-native and Bevy-ecosystem "control layer" options —
> anything that lets external code drive a running Bevy app — and assess each
> against (a) the project's actual Bevy version and (b) the specific control
> surface the e2e harness needs (enumerated in `00-reuse-audit.md` §
> "In-process functional-control surfaces"). **No solution design** — assess
> only.
>
> Every compatibility claim below is relative to the confirmed pinned Bevy
> version in § 1. Web sources are cited inline; anything unconfirmable is
> marked **unconfirmed**.

---

## 1. Bevy version — confirmed

**The project is pinned to `bevy = "=0.19.0-rc.1"`** (exact `=` pin, not a
caret range). Verified in three places, all in
`crates/bevy_naadf/Cargo.toml`:

- base `[dependencies]` — `bevy = { version = "=0.19.0-rc.1", features = ["free_camera"] }` (`:48-50`)
- `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` — adds `["asset_processor"]` (`:132-133`)
- `[target.'cfg(target_os = "android")'.dependencies]` — adds `["android-game-activity"]` (`:140-141`)

Cargo merges the per-target feature sets. **Effective Bevy feature set per target:**

| Target | Bevy features enabled |
|---|---|
| native (Linux/desktop) | `free_camera`, `asset_processor` |
| Android | `free_camera`, `asset_processor`, `android-game-activity` |
| wasm32 | `free_camera` (+ `webgpu` if `--features webgpu`) |

The crate's own `[features]` (`:212-224`) are `default = []`, plus opt-in
`dlss`, `force_disable_dlss`, `webgpu` — none touch remoting.

**The audit's claim is correct: `bevy_remote` / the `remote` feature is OFF.**
Bevy 0.19's `remote` cargo feature is not in any of the three feature lists.
`git grep` in the audit found zero `bevy_remote`/`RemotePlugin`/`brp` matches
in source. BRP is entirely absent today; enabling it is adding `"bevy_remote"`
to the Bevy feature list (a one-line change) plus adding `RemotePlugin` to the
app.

The user said "0.19 or at least 0.18" — **the project is on 0.19** (specifically
the first release candidate). Every option below is judged against 0.19-rc.1.
Note the consequence: the project is on a *release candidate*, so it is ahead
of most of the ecosystem — the third-party crates surveyed below mostly top out
at Bevy 0.18, one minor version behind.

---

## 2. Options table

One row per surveyed control-layer option. "Bevy-version compat" is relative to
the confirmed `0.19.0-rc.1`.

| Option | Latest version | Bevy compat | Maintenance | Control surface exposed | Transport(s) |
|---|---|---|---|---|---|
| **`bevy_remote` (BRP) — first-party** | ships *inside* `bevy` 0.19-rc.1 as the `bevy_remote` crate / `remote` feature | **Native match — it IS Bevy 0.19.** | First-party, maintained in-tree by the Bevy project. | ECS-level: entity/component spawn/get/insert/remove/mutate/query/reparent/list; resource get/insert/remove/mutate/list; `world.trigger_event`; schedule list/graph; registry schema; `rpc.discover`. **Plus arbitrary custom verbs** via `with_method_*`. See § 3. | Transport-agnostic core; `RemoteHttpPlugin` ships in-tree (HTTP, default port 15702, JSON-RPC 2.0, optional WebSocket upgrade). HTTP transport is **native-only** (gated `not(wasm)`). Core is a channel mailbox — a custom non-HTTP transport plugin is a supported extension point. |
| **`bevy_brp_extras`** | 0.19.0 (crate's own version; release ~2026-03) | **Bevy 0.18** (docs.rs: "bevy_brp_extras 0.18.x–0.19.x" → Bevy 0.18). **Not confirmed on 0.19-rc.1.** | Actively maintained (natepiano/bevy_brp), frequent releases. | Adds ready-made BRP methods the engine doesn't ship: `brp_extras/screenshot`, `shutdown`, `set_window_title`, `get_diagnostics`, keyboard (`send_keys`, `type_text`), mouse (`click_mouse`, `move_mouse`, `drag_mouse`, `scroll_mouse`, …). Composes with an existing `RemotePlugin`/`RemoteHttpPlugin`. | Inherits BRP transport (HTTP via `RemoteHttpPlugin`). |
| **`bevy_brp_mcp`** | 0.19.0 (crate's own version) | targets the same Bevy as `bevy_brp_extras` → **Bevy 0.18**, unconfirmed on 0.19-rc.1 | Actively maintained (same author). | Not a control layer for the app — an **MCP server** that *launches* Bevy apps and proxies BRP to an AI assistant. Adjacent, not in scope as a harness driver, but its "spawn the app + talk BRP to it" shape is exactly the SUT-driver pattern. | stdio (MCP) on one side, BRP/HTTP on the other. |
| **`bevy_mod_scripting`** | 0.19.0 (crate's own version) | **Bevy 0.18** ("0.19.0 compatible with Bevy 0.18.0+"). Not on 0.19-rc.1. | Actively maintained (makspll), recently rewritten; Rune support temporarily on hold. | In-process Lua/Rhai scripting: scripts call into the ECS (spawn, query, mutate, call registered functions). It is an *in-app* scripting surface — driven by script assets, **not** an external-process control channel. Would still need a transport to be driven externally. | None external — script files / assets loaded by the app. |
| **Bevy Integration Testing Toolkit (BITT)** | 0.5 | **Bevy 0.13** (BITT 0.5 → Bevy 0.13). 6 minor versions behind. | Master's-thesis project, minimally maintained, no recent activity. | Input record/playback, before/after screenshots, `Asserter::pass` completion marker. Purpose-built "drive a Bevy app from a test" — but the closest-to-our-need crate is also the most stale. | In-process plugin; no external transport. |
| **`bevy-test-suite`** | early/0.x | **unconfirmed** (claims a `headless` config; Bevy version not verified — treat as unknown) | small crate, low activity | `#[bevy_test]` attribute macro + `TestApp` trait: spawn entities, `app.advance_time()` frame stepping, headless option. | In-process — same-process test harness, no external transport. |
| **`bevy_geppetto`** | 0.x prototype | **unconfirmed**, old | prototype, low activity | Snapshot testing; tests run on main thread for winit; input + screen capture. | In-process. |
| **Bevy built-in: `App::update()` / `App::run()`** | n/a (engine API, 0.19-rc.1) | **exact match** | first-party | Programmatic single-frame stepping (`app.update()` advances one frame) and full ECS access (`app.world_mut()`) — but **same-process only**. The 7 already-headless `validate_*` gates in the audit already use this. | None — in-process. |
| **Bevy built-in: `ScheduleRunnerPlugin`** | n/a (engine API, 0.19-rc.1) | **exact match** | first-party | Headless run loop with `RunMode::Once` / `RunMode::Loop` — drives `App` without a winit window. Relevant for headless SUT boot, **not** a remote control channel. (Could not fetch the rc.1 docs page directly — the `RunMode` enum and `run_once`/`run_loop` constructors are long-standing Bevy API, **treat exact rc.1 signatures as unconfirmed**, but the plugin's existence and role are stable.) | None — in-process. |
| **Custom RPC-over-IPC (the proposed greenfield)** | n/a | n/a | n/a (would be project-authored) | Whatever the project writes. | Whatever the project chooses (Unix socket / pipe / TCP) — new deps required (no `tokio`/`interprocess`/`bincode`/`tarpc` in the tree per the audit). |

---

## 3. BRP deep-dive

This is the load-bearing section: if first-party BRP covers enough of the
surface, "custom RPC" largely collapses into "custom BRP methods + a transport
choice." Sources for this section: the `bevy_remote` crate `lib.rs` and
`Cargo.toml` at the **`v0.19.0-rc.1` git tag** (i.e. exactly the project's
pinned Bevy), plus the official `bevy::remote` module docs.

### 3.1 What BRP is

BRP is Bevy's first-party remote-control protocol: a **JSON-RPC 2.0**,
request/response protocol between a Bevy app (server) and a client. The client
always initiates; the server only responds. The protocol is documented as
**transport-agnostic and serialization-agnostic**. It moves toward OpenRPC
service-discovery support in the 0.19 cycle.

`RemotePlugin` sets up the protocol machinery **without starting any
transport**. A second plugin (e.g. `RemoteHttpPlugin`) attaches an actual
transport. Internally, requests flow through a channel mailbox: `BrpSender` →
`BrpReceiver` resources; the `process_remote_requests` system drains the
mailbox each frame in the `RemoteLast` schedule. **That channel mailbox is the
transport-agnostic seam** — a custom transport plugin just needs to push
`BrpMessage`s into `BrpSender`.

### 3.2 Built-in verbs (method-name strings, from `v0.19.0-rc.1` source)

Entity / component:
- `world.get_components` — read component values from an entity
- `world.query` — query the ECS with component filters
- `world.spawn_entity` — create an entity with components
- `world.despawn_entity` — despawn an entity
- `world.insert_components` / `world.remove_components` — add / delete components
- `world.mutate_components` — mutate a field within a component
- `world.reparent_entities` — reassign parentage
- `world.list_components` — enumerate registered or entity-present components
- `world.get_components+watch`, `world.list_components+watch` — streaming watchers

Resources:
- `world.get_resources` — read a resource value
- `world.insert_resources` — add / update a resource
- `world.remove_resources` — delete a resource
- `world.mutate_resources` — mutate a field within a resource
- `world.list_resources` — list reflectable resource types

Events / discovery / introspection:
- `world.trigger_event` — emit an event (takes the event's fully-qualified type
  name + value)
- `world.write_message` — send a message
- `world.observe` — watch entity/component state changes
- `registry.schema` — schema info on registered types
- `schedule.list`, `schedule.graph` — enumerate schedules / dump schedule graph
- `rpc.discover` — OpenRPC method discovery

**Hard constraint on every built-in verb:** they operate on **reflected** types.
A resource is only reachable by `world.get_resources` / `world.mutate_resources`
if it is `#[derive(Reflect)]` and registered with `app.register_type::<T>()`.
The project's config resources (`E2eGateMode`, `GiSettings`, `GridPreset`,
`ConstructionConfig`, `TaaConfig`, …) are `#[derive(Resource)]` per the audit —
**the audit does not state whether they also derive `Reflect`**. If they do
not, the built-in resource verbs cannot touch them until they do. This is a
concrete, checkable prerequisite, flagged in side-notes.

### 3.3 Custom-method registration — the extensibility hook

This is the mechanism that makes BRP an alternative to bare custom RPC. In
`0.19.0-rc.1` the API is **split by world** (this is a 0.19-era refinement;
older docs show a single `with_method`):

At plugin construction (builder, chainable):
```rust
RemotePlugin::default()
    .with_method_main(name, handler)            // runs in the MAIN world
    .with_method_render(name, handler)          // runs in the RENDER world
    .with_watching_method_main(name, handler)   // streaming, main world
    .with_watching_method_render(name, handler) // streaming, render world
```
Exact signatures verified at the `v0.19.0-rc.1` tag:
```rust
pub fn with_method_main<M>(
    self, name: impl Into<String>,
    handler: impl IntoSystem<In<Option<Value>>, BrpResult, M>) -> Self
pub fn with_method_render<M>(
    self, name: impl Into<String>,
    handler: impl IntoSystem<In<Option<Value>>, BrpResult, M>) -> Self
```

At runtime, via the `RemoteMethods` resource:
```rust
RemoteMethods::insert(
    &mut self,
    method_name: impl Into<String>,
    handler: RemoteMethodSystemId,   // ::Instant(SystemId) | ::Watching(SystemId)
) -> Option<RemoteMethodSystemId>
```
(register the system first via `world.register_boxed_system()`).

**Handler signature** — a custom method is an ordinary Bevy system:
```rust
fn handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult
```
It takes optional JSON params, gets **exclusive `&mut World` access**, and can
also use arbitrary system params. Watching variants return
`BrpResult<Option<Value>>` (return `None` for "no change this frame").

**Significance:** a custom method has full `&mut World` access. Anything the
project can do in a Bevy system — call a brush function, flip an enum resource,
trigger a screenshot, advance a frame counter, read a verdict resource — is
expressible as a custom BRP method. The `with_method_render` variant matters
specifically for this project: framebuffer readback lives in the render world
(`e2e/checks.rs` `PipelineScanResult` is cloned into the `RenderApp`), and BRP
0.19 can register a method **directly in the render world**.

### 3.4 Transports

- **`RemoteHttpPlugin`** — HTTP, JSON-RPC 2.0, default port 15702. In-tree,
  behind the `bevy_remote` crate's `http` feature (which is **on by default**
  for `bevy_remote`). Deps it pulls: `async-io`, `hyper`, `smol-hyper`,
  `http-body-util` (all native-only). Optional WebSocket upgrade on the same
  endpoint.
- **`RemoteHttpPlugin` is native-only** — gated `not(target_family = "wasm")`.
  There is **no first-party wasm BRP transport**.
- **No first-party stdio / Unix-socket / named-pipe transport.** The only
  documented non-HTTP example is `bevy_mcp` bridging stdio↔BRP, and it does so
  by talking HTTP to `localhost:15702` — i.e. it does *not* replace the
  transport, it wraps it. A genuine stdio/IPC BRP transport would be a custom
  plugin pushing into `BrpSender` — **but that custom transport is far smaller
  than a whole custom RPC stack**: it is one plugin feeding an existing,
  first-party request dispatch + JSON-RPC framing + method registry.

### 3.5 What BRP gives for free vs what needs a custom method

| Harness need | BRP off-the-shelf? |
|---|---|
| get/set a resource (`E2eGateMode`, `GiSettings`, …) | **Free** via `world.get_resources` / `world.insert_resources` / `world.mutate_resources` — **iff** the resources derive `Reflect` + are `register_type`'d. Otherwise a 1-line custom method per resource (or just derive `Reflect`). |
| query / spawn / mutate entities | **Free** — entity verbs are BRP's core. |
| trigger an event | **Free** — `world.trigger_event`. |
| **step / advance frames deterministically** | **Not a built-in verb.** BRP requests are *drained once per frame* by `process_remote_requests`; BRP does not pump the schedule. A custom `In(...)`-`&mut World` method could call frame-advance logic, but deterministic stepping (run-app-N-frames-then-respond) fights BRP's one-request-per-frame model — see § 5. **Thin custom method, with a caveat.** |
| **trigger a framebuffer screenshot capture** | **Not a built-in verb.** Either a thin custom method that spawns `Screenshot::primary_window()` (the project already wraps this in `e2e/readback.rs`), or adopt `bevy_brp_extras`'s ready-made `brp_extras/screenshot` (Bevy-0.18, **unconfirmed on 0.19-rc.1**). **Thin custom method.** |
| **invoke a voxel brush** | **Not a built-in verb** — there is no "call this function" generic verb. A thin custom `with_method_main` handler with `&mut World` calls `sphere_brush(&mut WorldData, …)` directly. **Thin custom method.** |
| read back a pass/fail verdict | **Free-ish** — if `E2eOutcome.gate_result` is a `Reflect` resource, `world.get_resources` reads it; otherwise a 1-line custom getter. |
| configure boot-time determinism knobs | **Not addressable by BRP at all** for the *boot-time* ones (`synchronous_pipeline_compilation`, fixed window size, `WinitSettings::Continuous`) — these must be set before `app.run()`, and BRP only exists after the app is running. See § 4 + § 5. |

### 3.6 BRP wasm / non-native story

The protocol core is portable, but **the only shipped transport (`RemoteHttpPlugin`)
is native-only**. There is no first-party BRP transport for `wasm32-unknown-unknown`.
For this project that means BRP cleanly covers the **native** e2e gates but
gives **nothing** for the wasm/Android e2e columns the audit's side-notes
already flag as out-of-scope-today. Android is native code (`libbevy_naadf.so`)
so `RemoteHttpPlugin` *could* run there over a TCP loopback if Android e2e is
ever built; wasm would need a custom transport (e.g. a `postMessage`/WebSocket
bridge) regardless of the BRP-vs-custom-RPC decision.

---

## 4. Required control verbs — the surface to cover

From `00-reuse-audit.md` § "In-process functional-control surfaces" + §
"Determinism risk", the harness must drive, from an external process:

1. **spawn** — start the production `bin/bevy-naadf` as a subprocess (the SUT).
2. **configure-determinism** — set boot-time knobs: `synchronous_pipeline_compilation`,
   fixed 256² window, `WinitSettings::Continuous`, 96-frame GI warmup budget.
   These are set inside `bootstrap::BootstrapInputs` *before* `app.run()`.
3. **step-frames** — advance the app a deterministic number of frames.
4. **capture-framebuffer** — trigger a screenshot, get pixels / PNG out.
5. **get/set-resource** — read & write config resources (`E2eGateMode`,
   `GiSettings`, …) and the verdict resource (`E2eOutcome.gate_result`).
6. **invoke-brush** — call `paint_brush` / `cube_brush` / `sphere_brush` on
   `WorldData`.
7. **read-verdict** — pull the pass/fail `Result` back out.

---

## 5. Coverage matrix

Surveyed options × required verbs. Cells: **C** = covered off-the-shelf ·
**T** = thin custom method needed · **N** = not possible with this option ·
**—** = out of scope for this option.

Only the three options that are actually live candidates for *this* job are
matrixed (BRP, custom RPC, and the in-process built-ins as a baseline). The
ecosystem testing/scripting crates are excluded from the matrix because none
of them target 0.19-rc.1 *and* offer an external-process channel — see § 6.

| Verb | `bevy_remote` (BRP) + transport | Custom RPC-over-IPC | Bevy built-in `App::update()` (in-process) |
|---|---|---|---|
| **spawn** (start SUT subprocess) | — *(out of BRP's scope; the runner does `std::process::Command`, same as today's compare gates)* | — *(runner-side; identical)* | N/A — same process, no spawn |
| **configure-determinism** (boot-time knobs) | **N** *(BRP exists only post-`run()`; knobs are pre-`run()` `BootstrapInputs` fields. Must be passed at spawn — CLI arg / env var — regardless of control layer)* | **N** *(same — pre-`run()`)* | **C** *(test sets `BootstrapInputs` directly before building the App)* |
| **step-frames** | **T** + caveat *(custom method; but BRP drains 1 request/frame — "run N frames then reply" needs either a watching method that counts down, or the runner issuing N no-op calls. Not free; semantically awkward)* | **T** *(design a `tick(n)` verb with whatever frame semantics you want — full control)* | **C** *(`app.update()` is literally one frame; the 7 headless validators already do this)* |
| **capture-framebuffer** | **T** *(custom `with_method_main` spawning `Screenshot`; or `bevy_brp_extras/screenshot`, Bevy-0.18 unconfirmed on 0.19)* | **T** *(custom verb wrapping `e2e/readback.rs`)* | **C** *(in-process: call `shoot_primary_window` + read `Framebuffer` directly)* |
| **get/set-resource** | **C** *(if `Reflect`+registered)* / **T** *(if not — 1-line getter/setter, or derive `Reflect`)* | **T** *(custom get/set verbs)* | **C** *(`world.resource_mut::<T>()` directly)* |
| **invoke-brush** | **T** *(custom `with_method_main` calling `sphere_brush`)* | **T** *(custom `apply_brush` verb)* | **C** *(call the brush fn directly with `&mut WorldData`)* |
| **read-verdict** | **C** / **T** *(as get-resource)* | **T** *(custom verdict verb)* | **C** *(read the resource directly)* |

**Reading of the matrix:** for the external-process requirement, the live
choice is BRP vs custom RPC. The in-process column is shown only as a baseline —
it covers everything trivially but **is not an option for the stated goal**
(the goal is explicitly external-process control of the *production* binary).
BRP and custom RPC have *the same shape* in the matrix — both leave
configure-determinism out (it is pre-`run()` for everyone), both make
step/capture/brush thin custom methods. **The decisive difference is not
coverage — it is how much of the plumbing already exists.** BRP brings the
JSON-RPC framing, the request dispatcher, the method registry, the per-frame
mailbox drain, the `In(params)/&mut World` handler model, the watching/streaming
model, and a working HTTP transport — all first-party, all maintained in-tree,
all already at exactly version 0.19-rc.1. Custom RPC brings none of that.

---

## 6. Why the ecosystem testing/scripting crates do not change the picture

- **`bevy_mod_scripting`** — Bevy 0.18, actively maintained, but it is an
  *in-app* scripting surface (scripts loaded as assets). To drive it from an
  external process you would still build a transport. It does not remove the
  RPC question; it relocates it. Out.
- **BITT** — the *only* crate purpose-built for "drive a Bevy app from a test,"
  and it is on **Bevy 0.13** (6 minor versions stale), a thesis project with
  no recent activity, and its model is input record/playback (it even warns
  real inputs leak through during playback). Porting it forward to 0.19-rc.1
  would be more work than it saves. Out.
- **`bevy-test-suite` / `bevy_geppetto`** — same-process test harnesses
  (`#[bevy_test]` macro, snapshot testing). They help write `tests/`-style
  in-process tests; they do **not** provide external-process control of a
  separately-spawned binary. Bevy-version support unconfirmed. They are
  relevant only to the audit's separable "Option C" (move the 9 headless
  validators to `tests/`), not to the 13 booted-window gates this restructure
  targets. Out of *this* question's scope.
- **`bevy_brp_extras` / `bevy_brp_mcp`** — these are *BRP accelerants*, not
  alternatives. `bevy_brp_extras` would hand the project a ready-made
  `screenshot` + input-injection method set — genuinely useful — **but it is
  built for Bevy 0.18 and unconfirmed on 0.19-rc.1**. Depending on it means
  either waiting for a 0.19 release or vendoring its handful of methods. They
  reinforce the BRP recommendation rather than competing with it.
- **`bevy-inspector-egui`** — local in-app UI inspector, no remote/external
  control surface. Not relevant.

---

## 7. Recommendation

**Custom RPC-over-IPC, as a from-scratch stack, is the wrong default. Reframe
the work as "custom BRP methods on a chosen BRP transport."**

The user's instinct is correct — there is a first-party Bevy control layer, and
it is not one minor version behind, it *is the project's exact Bevy version*
(`bevy_remote` ships inside `bevy 0.19.0-rc.1`). BRP already provides every
piece of a custom RPC stack that is generic plumbing:

- JSON-RPC 2.0 wire format + request/response framing
- a request dispatcher and a method registry (`RemoteMethods`)
- a per-frame mailbox drain (`process_remote_requests` in `RemoteLast`)
- the `In(Option<Value>)` + `&mut World` handler model — every project verb is
  an ordinary Bevy system with full world access
- a streaming/watching model (`with_watching_method_*`)
- a working, maintained HTTP transport (`RemoteHttpPlugin`)
- main-world **and** render-world method registration (`with_method_render`) —
  directly relevant since this project's framebuffer readback is render-world

Building all of that by hand is the wheel-reinvention the user is pushing back
on. The custom-method hook means the project's *domain* verbs (step, capture,
brush, gate-mode) are small `&mut World` systems registered with one builder
call each — not a parallel RPC framework.

**What genuinely remains as custom work under BRP** (this is the honest cost,
not zero):

1. **Domain verbs as custom BRP methods** — `step_frames`, `capture_framebuffer`,
   `apply_brush`, and getters/setters for any config resource that does not
   derive `Reflect`. Each is a thin `with_method_main`/`with_method_render`
   handler wrapping existing primitives (`e2e/readback.rs`, `editor/tools.rs`).
   This is irreducible: it exists under custom RPC too, and is *smaller* under
   BRP because the framing/dispatch is free.
2. **Frame-stepping semantics** — BRP drains one request per frame; "run N
   frames then reply" is awkward in that model. The architect must decide
   between a watching method that counts frames down, the runner issuing N
   calls, or a custom method that pumps the schedule. This is a real design
   question BRP does not answer for free — but it is *also* a real design
   question for custom RPC. BRP does not make it worse.
3. **Transport choice.** `RemoteHttpPlugin` (HTTP over loopback) works on native
   today with zero project code — that alone unblocks the 13 native gates. If
   the project specifically wants a non-network IPC channel (Unix socket /
   pipe / stdio), that is a **custom transport plugin** — but it is one plugin
   pushing `BrpMessage`s into `BrpSender`, *not* a custom protocol. The
   transport-agnostic channel mailbox is a first-party, supported seam. This is
   the only place "custom IPC" legitimately survives — and it is a fraction of
   the original scope.
4. **Boot-time determinism knobs are out of scope for any control layer.**
   `synchronous_pipeline_compilation`, fixed window size, `WinitSettings::Continuous`
   are `BootstrapInputs` fields set *before* `app.run()`. No post-`run()` RPC
   (BRP or custom) can set them. They must be passed at spawn time — CLI flag
   or env var on the SUT subprocess. The architect must not design an RPC
   `configure` verb for these; they belong to the spawn contract. (This is the
   audit's load-bearing determinism risk — it is real, and it is orthogonal to
   the transport decision.)

**Net:** the question "custom RPC or not" should be retired and replaced with
two narrower decisions: **(a)** which BRP transport — first-party
`RemoteHttpPlugin` over loopback (zero custom code, native-only) vs a custom
IPC/stdio transport plugin (small, if a non-network channel is required);
**(b)** how to express deterministic frame-stepping as a BRP method. Everything
else — the protocol, the dispatch, the verb-registration model, the
render-world hook — is first-party and already at 0.19-rc.1. The only honest
argument *for* a fully custom stack would be a hard requirement BRP cannot meet
(e.g. a binary wire format for multi-MB framebuffer payloads, or a wasm
transport) — and even those are addressed by a custom *transport* under BRP,
not a custom *protocol*.

---

## Side notes / observations / complaints

- **The brief's framing ("design a custom RPC-over-IPC layer") had already
  pre-judged the answer the user is now questioning — and the user is right to.**
  The whole exercise is salvageable: nearly every "custom RPC" concern is a
  custom-*method* or custom-*transport* concern, both of which are first-party
  extension points in `bevy_remote`. The architect should not produce an
  "Option A/B/C custom RPC framework" — per the project's port-as-is /
  no-speculative-redesign rules, the grounded move is "adopt BRP, enumerate the
  custom methods, pick a transport."

- **Unconfirmed-but-load-bearing: do the project's config resources derive
  `Reflect`?** BRP's built-in `world.get_resources`/`world.mutate_resources`
  only see `Reflect` + `register_type`'d types. The audit calls `E2eGateMode`,
  `GiSettings`, `GridPreset`, `ConstructionConfig`, `TaaConfig` `#[derive(Resource)]`
  but is silent on `Reflect`. **The architect must grep this before sizing the
  work** — if they are already `Reflect`, get/set-resource is free; if not,
  it's either a `Reflect` derive per type or a custom getter per type. Cheap to
  check, decisive for the matrix. I could not check it (read-only dispatch
  scoped to `Cargo.toml` + the audit) — flagging, not guessing.

- **The version-skew trap.** The project is on a *release candidate*
  (`0.19.0-rc.1`). The whole third-party ecosystem (`bevy_brp_extras`,
  `bevy_brp_mcp`, `bevy_mod_scripting`) tops out at Bevy **0.18**. First-party
  `bevy_remote` is the *only* surveyed control layer that is, by construction,
  at exactly the project's version — because it ships inside `bevy` itself.
  This is a strong structural argument for BRP that goes beyond feature
  coverage: a project pinned to an rc has no margin for a dependency that lags
  a minor version. The flip side: BRP itself is rc-grade in 0.19 (the protocol
  is "moving toward OpenRPC" — actively churning). The `with_method_main` /
  `with_method_render` *split* is itself a 0.19-era change from the older
  single `with_method` — so any design must be written against the
  `v0.19.0-rc.1`-tagged API, not stale tutorials. Both fetches in this survey
  that hit "latest" docs.rs returned older single-`with_method` API; the
  per-world split is confirmed only from the `v0.19.0-rc.1` git tag.

- **The real blocker is not transport — it is determinism + frame-stepping.**
  Re-reading the audit's "Determinism risk" side-note: the hard part of this
  restructure is that the production app is `AppConfig::windowed()` and the
  e2e determinism (sync pipeline compile, fixed window, continuous winit,
  96-frame GI warmup) all comes from `AppConfig::e2e()`. No control layer —
  BRP or custom — touches the pre-`run()` boot config. And frame-stepping
  fights BRP's one-request-per-frame mailbox model. **If the orchestration
  spends its design budget on "which RPC transport," it is optimising the
  easy 20%.** The transport is nearly a solved problem (`RemoteHttpPlugin`
  exists). The 80% is: (1) how the SUT receives boot-time determinism config
  at spawn, and (2) how "advance exactly N frames, deterministically, then let
  me assert" is expressed over a request/response protocol whose server only
  services requests once per frame. The brief points the architect at
  transport; the architect should point themselves at determinism.

- **wasm and Android are genuinely uncovered and the brief under-asks here.**
  `RemoteHttpPlugin` is native-only. The audit already notes there is no
  Android e2e today and the Playwright wasm parity spec is cross-target. BRP
  cleanly serves the 13 native booted gates and nothing else. If "all e2e
  scenarios on all targets over one RPC schema" is ever a goal, the wasm
  column needs a custom transport (postMessage/WebSocket bridge) — but that is
  true under custom RPC too, and is a separate, later problem. The architect
  should explicitly scope this restructure as **native-only** unless told
  otherwise.

- **`bevy_brp_extras` is a tempting shortcut that the version pin probably
  forbids.** It ships exactly the missing pieces (`screenshot`, `send_keys`,
  `click_mouse`, …) as ready BRP methods — but for Bevy 0.18, unconfirmed on
  0.19-rc.1. The architect should check whether a 0.19-compatible release
  exists; if not, the realistic path is vendoring the two or three methods the
  harness actually needs (screenshot is the main one, and the project already
  has `e2e/readback.rs` doing exactly that — so even vendoring is unnecessary).
  Do not take a hard dependency on a 0.18 crate from a 0.19-rc project.
