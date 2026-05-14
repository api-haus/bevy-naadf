# e2e-render-test — windowed end-to-end render test binary

## delegate-architect findings (2026-05-14)

A design for a **single deterministic windowed end-to-end rendering invocation** that boots the
real Bevy app (real `WinitPlugin`, a real on-screen window, real `RenderDevice`, real render
graph, real WGSL pipeline creation), runs the render graph for a fixed frame count, reads the
final framebuffer back to the CPU, asserts per-batch visual gates, and exits — non-zero on
failure, zero on success. It **replaces the open-ended live `cargo run` smoke-run as the impl
agent's verification step** (see §10).

It is a **dedicated binary target** `src/bin/e2e_render.rs`, invoked as
**`cargo run --bin e2e_render`** — a single command that opens a window, renders the scene for a
bounded number of frames, then ends on its own. It is **not** a `#[test]` (see §2.1 for why the
winit main-thread constraint forces a binary, not the `cargo test` harness).

Worktree: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/phase-b-gi`, branch `feat/phase-b-gi`.
The crate is currently binary-only with one binary (`bevy-naadf`, from `src/main.rs`); this adds
a second binary (`e2e_render`). The existing `cargo test --bin bevy-naadf` unit suite is
**unchanged** — it stays as it is (pure-CPU, fast). The e2e invocation is its own command, run
once, alongside `cargo build` + `cargo test`.

Every Bevy API named below is verified by Read against the vendored crate source at
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_*-0.19.0-rc.1/`. Every repo path /
symbol is verified by Read/Grep against the worktree.

---

## 1. The problem this solves (why it is not optional)

`cargo build` and `cargo test` compile only Rust. The WGSL shaders in `src/assets/shaders/**`
are **runtime assets**: naga-oil composes them and the GPU driver validates/compiles them only
at *pipeline-creation time*, on the first rendered frame. Every shader-class bug the Phase-B
impl log already hit —

- naga-oil trailing-digit field-name rejection (`10-impl-b.md` Batch 1 bug #1, Batch 3 bug #1),
- `ptr<storage,…>` function-parameter rejection (Batch 1 bug #2),
- `vec2<u32>`-from-`f32` constructor rejection (Batch 2 bug #1),
- `let _ = <imported-fn>(…)` post-rewrite rejection (Batch 3 bug #2),
- scalar↔vec broadcast type errors (Batch 3 bug #3),
- uniform-struct `vec3`-then-scalar layout mismatch → GPU TDR (Batch 1 bug #3),

— is **invisible to `cargo build`/`cargo test`** and only surfaces on a live windowed run. Worse,
each run aborts (or the node silently `return`s — see §3) on the *first* bad pipeline, so
clearing N shader bugs costs N+ slow window-opening runs. Batch 1 alone cost the impl agent
~10 reruns at ~a minute each.

The thing that was wrong with the old loop was **not** that a window appeared — it was that the
agent re-ran an *open-ended* app (no exit, watch for ~30 s, kill it, repeat). This harness keeps
the real window — that is deliberate and correct — but makes the run **bounded and
deterministic**: it boots, forces **every** Phase-B pipeline to be created, renders a fixed
number of frames, reads the framebuffer back, runs the assertions, and **exits with a process
code**. The naga/wgpu error text lands in the exit. The impl agent runs it **once**, reads the
exit code + stderr, and is done — no loop.

---

## 2. Windowed boot mechanism (Bevy 0.19-rc.1 specifics)

### 2.1 Why a binary target, not a `#[test]`

winit requires its event loop to run **on the main thread** — that is a hard winit constraint,
not a Bevy one. `cargo test`'s `#[test]` harness runs each test function on a **worker thread**;
a real `WinitPlugin` window cannot be created or driven from there. So the e2e invocation
**cannot** be a `#[test]` in `tests/` or a `#[cfg(test)]` module — those would force the headless
mechanism the v1 design chose and the user rejected.

The fix is a **dedicated binary target**: `src/bin/e2e_render.rs`, with

```rust
fn main() -> AppExit {
    bevy_naadf::run_e2e_render()
}
```

`fn main() -> AppExit` works because `AppExit` implements `std::process::Termination`
(`bevy_app-0.19.0-rc.1/src/app.rs:1594-1601` — `AppExit::Success → ExitCode::SUCCESS`,
`AppExit::Error(n) → ExitCode::from(n)`). The process exit code therefore *is* the test result,
with **zero glue**: return the `AppExit` that `app.run()` produced (§2.4) and the kernel sees
0 on success, non-zero on failure. The binary's `main` runs on the process main thread, so
`WinitPlugin` is happy.

Invocation for the impl agent: **`cargo run --bin e2e_render`** — one command, runs once, exits.
(Cargo auto-discovers `src/bin/*.rs` as binary targets; no `Cargo.toml` `[[bin]]` entry is
required, though an explicit one is harmless.)

### 2.2 Plugin set — the real `DefaultPlugins`, identical to `main.rs`

The e2e binary builds the **same app as `main.rs`** — the real `DefaultPlugins` (which includes
`WinitPlugin` — `bevy_internal-0.19.0-rc.1/src/default_plugins.rs:40`), the real
`AssetPlugin { file_path: "src/assets" }`, `world::WorldPlugin`, `render::NaadfRenderPlugin`,
`FrameTimeDiagnosticsPlugin`, `RenderDiagnosticsPlugin`. A real winit window opens; the entire
`Core3d` graph runs against a real window surface `ViewTarget` exactly as in production. This is
the whole point of the windowed choice: the test exercises the *actual* boot path, not a
near-copy of it.

The e2e app differs from `main.rs` in exactly four deliberate, minimal ways, all carried by an
`E2eConfig` passed into the shared `build_app` (§9):

1. **`RenderPlugin { synchronous_pipeline_compilation: true, .. }`** — set this on the e2e
   config. With it, `PipelineCache` blocks until each queued pipeline reaches `Ok` or `Err`
   within the same `app.update()` (`bevy_render-0.19.0-rc.1/src/lib.rs:129-133,463`). Without
   it, pipeline creation is a background `Task` and "did all pipelines compile?" becomes a race
   against the frame count. Synchronous compilation makes the run **deterministic** and lets a
   small fixed frame budget guarantee every pipeline is resolved. The windowed `main.rs` app
   keeps async compilation (no startup hitch) — `E2eConfig` parameterises this so only the e2e
   binary flips it on (`09-design-b.md`-style per-`RenderPlugin` flag, not global).
2. **`WinitSettings { focused_mode: UpdateMode::Continuous, unfocused_mode: Continuous }`** —
   insert this resource. The default `WinitSettings` drops to `reactive_low_power` when the
   window loses focus (`bevy_winit-0.19.0-rc.1/src/winit_config.rs:20-21`); the e2e window may
   never gain focus on a busy desktop, and `Reactive` mode only ticks on events — which would
   stall the bounded frame loop. `Continuous` in both modes guarantees the app ticks every
   frame regardless of focus, so the fixed frame budget (§4) advances deterministically. (This
   is a one-line resource insert, the documented `WinitSettings::game()` shape.)
3. **No HUD.** `setup_hud` / `update_hud` are omitted from the e2e config — the HUD is a UI
   overlay drawn over the blit; it is irrelevant to the render gates and pulls in font assets.
   (`RenderDiagnosticsPlugin` / `FrameTimeDiagnosticsPlugin` *are* kept — §8 reads their
   `DiagnosticsStore` for the node-dispatch check.)
4. **No `FreeCameraPlugin`, fixed camera, bounded-frame driver + assertion systems added.** The
   e2e config omits `FreeCameraPlugin` (the camera is static by design — §4.2 — and `FreeCamera`
   is not read by any render system) and adds the §4 bounded-frame driver + the §6 assertion
   systems. The window is real and on-screen, but the camera does not move and the run is
   self-terminating.

DLSS: the e2e binary does **not** insert `DlssProjectId` and does not add DLSS camera
components — Phase B keeps DLSS dormant (`01-context.md` §2d). `DlssProjectId` is only consulted
if inserted, so simply not inserting it is enough; `--no-default-features` is not required.

### 2.3 GPU requirement — unconditional

This run needs a real GPU adapter (the default `RenderCreation::Automatic` backend). **That is
fine and requires no special handling** — there is a GPU (RTX 5080) on the only machine this
runs on, and there is no CI to accommodate. The e2e binary simply requires a GPU and always
runs. No feature gate, no runtime self-skip, no GPU-detection branch — none of that machinery
exists in this design. If there were ever no GPU the binary would fail at adapter creation,
which is the correct and obvious outcome.

### 2.4 How the run ends — `AppExit` through the winit runner

This is the mechanism that makes a *windowed* app a *bounded single invocation*:

- The winit runner (`bevy_winit-0.19.0-rc.1/src/state.rs:885` `winit_runner`) checks
  `self.app.should_exit()` every iteration (`state.rs:735-737`) and, when any system has written
  an `AppExit` message, **exits the winit event loop and returns that `AppExit`** from
  `app.run()`.
- So the e2e app adds a system that, after the fixed frame budget + the readback drain (§4, §5),
  **writes an `AppExit`** — `AppExit::Success` if every gate passed, `AppExit::error()` (or
  `AppExit::from_code(n)`) if any gate failed or any error was detected.
- `app.run()` returns that `AppExit`; `run_e2e_render()` returns it; `fn main() -> AppExit`
  reports it as the process exit code.

Net: a real window opens, renders the scene for ~12 frames, the assertions run, the window
closes, and the process exits 0 or non-zero — one `cargo run --bin e2e_render`, no loop, no
manual kill.

A panic inside `app.update()` (a `DeviceLost`, a failed `queue.submit`) propagates up through
the winit runner and aborts the process with a non-zero code and the wgpu message on stderr —
that is also a correct failure (§3.2).

---

## 3. Catching shader / pipeline / bind-group / validation errors

This is the harness's primary value. Two independent detection layers, because the failure
modes are different:

### 3.1 Layer A — `PipelineCache` error-state scan (the main check)

The render-graph nodes use the pattern
`let Some(pipeline) = pipeline_cache.get_compute_pipeline(id) else { return; };`
(`graph.rs:76-80`, `graph.rs:146-150`, `graph.rs:200-202`, and every node in `graph_b.rs`).
`get_compute_pipeline` / `get_render_pipeline` return `Option` and yield `None` for **both**
the not-yet-ready (`Queued`/`Creating`) state **and** the `Err` state
(`bevy_render-0.19.0-rc.1/src/render_resource/pipeline_cache.rs:336,370`). A failed pipeline
therefore makes the node *silently skip* — no panic. So a shader bug currently produces a
black/stale frame, **not** a crash. The harness cannot rely on a panic.

Instead, after the fixed frame loop, an assertion system reaches into the `RenderApp` sub-world
and **iterates `PipelineCache::pipelines()`** (`pipeline_cache.rs:221` — public, yields
`&CachedPipeline`; `CachedPipeline { descriptor, state }` at `:39-42`). For each pipeline whose
`state` is `CachedPipelineState::Err(ShaderCacheError)` (`:46-55`), it collects
`(descriptor label / shader path, error)` and the run **fails with the full list** — `eprintln!`
the list and write `AppExit::error()`. The `ShaderCacheError` `Display` carries the naga-oil /
wgpu validation message — the same text the live run logs via `error!` (`pipeline_cache.rs:699-705`).

Accessing the `RenderApp` world from a main-world system or from `run_e2e_render` after
`app.run()` returns:
```rust
let render_app = app.sub_apps_mut().get_mut(RenderApp).unwrap();   // app.rs:1196 sub_apps_mut
let cache = render_app.world().resource::<PipelineCache>();
```
(`PipelineCache` is a render-world resource; `app.sub_apps()` / `sub_apps_mut()` are public —
`bevy_app-0.19.0-rc.1/src/app.rs:1191-1197`.) Because this is a binary, not a `#[test]`, the
scan can run either as a Bevy system in the main world (using a one-shot system / a startup-gated
system that reads the render sub-app — see §6.5) **or** in `run_e2e_render` between `app.run()`
returning and the function returning its `AppExit`. The §11 plan uses the latter for the
pipeline-scan and degenerate-frame checks (simplest, no sub-app-access-from-system plumbing) and
a main-world system only for the parts that need per-frame timing (the §6.4 consecutive-frame
delta).

**Why this catches everything in the bug list:** naga-oil composition failures, WGSL validation
failures, bind-group-layout / pipeline-layout mismatches, and `ptr<storage>`-param rejections
all land in `CachedPipelineState::Err` at pipeline-creation time. With
`synchronous_pipeline_compilation: true`, every pipeline the render systems queue is resolved
to `Ok` or `Err` within the frame budget — so the scan sees the true terminal state.

### 3.2 Layer B — wgpu device-error capture (catches the runtime-validation / TDR class)

Some failures are *not* pipeline-creation errors: a uniform-layout mismatch that compiles fine
but reads garbage (`10-impl-b.md` Batch 1 bug #3 — the `AtmosphereParams` `vec3`-then-scalar
TDR), an out-of-bounds dispatch, a bind-group that does not match the layout at *bind* time.
These surface as wgpu **device errors / validation errors** during command submission, or as a
`DeviceLost`. Bevy's `RenderDevice` wrapper does not expose a wgpu uncaptured-error scope to
test code (verified — no such API on the 0.19-rc.1 wrapper), so the harness relies on two
cheaper signals that are sufficient in practice:

1. **Panic propagation.** A `DeviceLost` or a failed `queue.submit` surfaces as a panic inside
   `app.update()`; in a windowed app that panic unwinds through the winit runner and **aborts
   the process with a non-zero exit code**, the wgpu message on stderr. (Batch 1 bug #3's TDR
   manifested as exactly this — `DeviceLost`/swapchain `Timeout`.) The binary does **not**
   catch-unwind; a panic in `update()` *is* the failure, and the non-zero abort is exactly the
   signal the impl agent reads.
2. **The readback sanity check (§7).** A pipeline that "compiled" but is mis-fed produces a
   degenerate framebuffer (all-zero, all-one, or unchanged-from-clear). The §7 assertions catch
   that as a content failure even when no error was raised.

Layer B is best-effort; Layer A is the load-bearing check. Together they cover the
`10-impl-b.md` bug catalogue.

### 3.3 Forcing every pipeline to be created

A pipeline is only queued when its render system runs and calls
`pipeline_cache.get_*_pipeline(id)` for the first time, AND the upstream resources exist (the
nodes early-return until `WorldGpu` / `FrameGpu` / `TaaGpu` / `AtmosphereGpu` / `GiGpu` exist —
`graph.rs:73-75`, `prepare.rs` resource gates). So "run a few frames" is not enough on its own —
the run must tick **enough frames that the prepare systems have built every resource and every
node has executed at least once**. Empirically that is small: the prepare systems build their
resources on the first valid frame, and `synchronous_pipeline_compilation` resolves the
pipelines the same frame they are queued. **Frame budget: 8 render frames** before the readback
is requested (§4) is comfortably above the resource-build latency (1 frame to extract the
world, 1 to prepare GPU resources, 1 for the first full graph execution) with margin for the
camera-history ring to spin up. If a future batch adds a pipeline that is only queued
conditionally, that batch's per-batch assertion (§6) must ensure the condition holds in the
test scene.

---

## 4. Frame loop & determinism

### 4.1 The bounded-frame driver — a counting system, not a manual `update()` loop

Because the run is driven by the real winit runner (§2.4), there is **no manual `app.update()`
loop**. Instead the e2e config adds a small **driver system** to `Update` that owns a frame
counter and a state machine:

```text
RUN  (frames 0..E2E_RENDER_FRAMES, = 8):  just count up — let the graph render & pipelines compile
SHOOT (one frame):                        spawn `Screenshot::primary_window()` + observer (§5)
DRAIN (frames 0..E2E_DRAIN_FRAMES, = 4):  count up, waiting for `ScreenshotCaptured` to fire
ASSERT (one frame):                       run the gates (§6), then write `AppExit::Success`/`error()`
```

The driver advances one state-step per `Update` tick. The winit runner ticks `Update` every
frame (`UpdateMode::Continuous`, §2.2). Total run length is ~`8 + 1 + ≤4 + 1 ≈ 14` frames — a
real window visible for well under a second, then it closes itself.

A monotonic per-run frame counter is needed anyway, and the project already has one:
`CameraHistory.frame_count` (`render/taa.rs`, incremented by `update_camera_history`). The
driver can read it directly, or keep its own `Local<u32>` — either way the count is a pure
integer, not wall-clock-derived.

### 4.2 Determinism strategy — exactly how

The run is a **single deterministic invocation**: same binary → bit-identical readback
framebuffer every run. Every non-deterministic input the render path consumes is pinned:

| input | how it is made deterministic |
|---|---|
| **Camera pose** | The e2e setup spawns the camera at a **fixed `Transform`** (a const in `e2e_render.rs` / the e2e module — e.g. the `setup_camera` pose `Transform::from_xyz(11,7,17).looking_at((0,4,-3), Y)` — `camera/mod.rs:40`, or a test-specific pose chosen to frame the gates in §6). `FreeCameraPlugin` is **omitted** (§2.2), so even though the window is real and can receive focus/input events, no system consumes them to move the camera — the `Transform` never changes. `sync_position_split` (`camera/position_split.rs`) is a pure function of the `Transform` → `PositionSplit` is deterministic. |
| **Frame counter** | `CameraHistory.frame_count` is a monotonic integer counter incremented by `update_camera_history` (`render/taa.rs` — `06-design-a2.md` §9; the `05-review.md` §4 wall-clock-millis bug was fixed in A-2). After N `Update` ticks it is exactly N. Not wall-clock-derived → identical every run. |
| **TAA jitter** | `halton_jitter(frame_count)` (`render/taa.rs`) is a pure function of the integer frame counter → deterministic. The jitter sequence is therefore identical run-to-run for a given frame index. **Do not disable TAA** — keep `AppArgs.taa = true` (the real path); determinism comes from the pinned frame counter, not from disabling jitter. (If a specific batch's gate proves jitter-sensitive at a region edge, that batch's assertion uses an interior region or a slightly relaxed tolerance — see §6 — rather than disabling TAA.) |
| **RNG / rand salt** | `GpuRenderParams.rand_counter` / `GpuGiParams.rand_counter*` are derived from `frame_count` (`prepare.rs`, `gi.rs` salt helpers) → deterministic given the pinned counter. The GI sampler's per-pixel noise is therefore the *same* noise every run. |
| **`Time` / `elapsed`** | No render-relevant system reads wall-clock `Time` (verified: `prepare.rs`, `taa.rs`, `position_split.rs`, `extract.rs` — none read `Time`/`elapsed` on the render path; the A-2 fix removed the last one). The winit runner advances `Time` per tick but nothing on the gate path consumes it. **One caveat the impl agent must honour:** the assertion gates run at a *fixed frame index*, not after a wall-clock delay — `Time`-based pacing must not creep into the driver. |
| **Window / viewport size** | The e2e setup spawns the window with a **fixed `Window { resolution: (E2E_WIDTH, E2E_HEIGHT).into(), .. }`** — a small fixed resolution (recommend **256×256** — large enough for stable regions in §6, small enough for a fast readback and fast GI dispatch; the window is also `resizable: false`). `extract_camera` reads `physical_viewport_size()` from the window surface (`extract.rs:128-131`) → fixed. All the `pixel_count`-sized buffers (`first_hit_data`, `final_color`, the GI buffers) are therefore a fixed size every run. (On a HiDPI desktop the physical size may be `scale_factor × logical` — pin `Window { resolution: WindowResolution::new(W,H), .. }` and, if the desktop applies scaling, the gate rectangles in §6 are derived from the *actual* physical readback dimensions, not assumed — see §6.5.) |
| **Test scene** | The existing `setup_test_grid` (`voxel/grid.rs`) builds the `GridPreset::Default` grid procedurally with **no RNG** (`build_default_volume` / `build_palette` are deterministic constructors). Reused as-is → the world geometry + the emissive block are bit-identical every run. |
| **Pipeline compilation** | `synchronous_pipeline_compilation: true` (§2.2) — no background-task race. |

Net: given the same binary, the readback framebuffer is bit-identical across runs. That is what
makes the §6 golden-vs-statistic decision a real choice rather than forced.

---

## 5. Framebuffer readback approach

### 5.1 The chosen mechanism — `Screenshot::primary_window()`, read back the real window

The camera's `RenderTarget` is the **real winit window** (the default — the e2e app does not
override it). To get the on-screen window framebuffer to the CPU the harness uses Bevy's
first-party **`Screenshot` component + `ScreenshotCaptured` observer**
(`bevy_render-0.19.0-rc.1/src/view/window/screenshot.rs`):

- During the `SHOOT` state-step (§4.1) the driver spawns
  `commands.spawn(Screenshot::primary_window()).observe(stash_screenshot)` (`screenshot.rs:80,98`).
  `Screenshot::primary_window()` targets `RenderTarget::Window(WindowRef::Primary)` — it reads
  back the **actual on-screen window surface**, the exact pixels the user would see.
- The screenshot is captured asynchronously; one or more frames later the renderer triggers
  **`ScreenshotCaptured { entity, image: Image }`** on the screenshot entity (`screenshot.rs:48-54`,
  `:210` `commands.trigger(ScreenshotCaptured { image, entity })`). The `image` is a full
  `Image` — `data: Option<Vec<u8>>` + `texture_descriptor` (format + size).
- The `stash_screenshot` observer stores the captured `Image` into a resource
  (`E2eScreenshot(Option<Image>)`); the driver's `DRAIN` state waits until that resource is
  populated, then transitions to `ASSERT`.

Why `Screenshot::primary_window()` over the v1 design's `GpuReadbackPlugin` + offscreen `Image`:
- The v1 readback path only worked because the v1 camera rendered to an **offscreen `Image`
  target** — that was the *headless* mechanism the user rejected. With a real window, the
  camera's target is the window surface, and a window swapchain surface texture is not directly
  `COPY_SRC`-mappable; `GpuReadbackPlugin`'s `Readback::texture` expects an `Image` handle, not
  a window. `Screenshot::primary_window()` is the API Bevy provides **specifically** to read
  back a window — it owns the surface-texture→`COPY_SRC`→`MAP_READ` handshake internally.
- It reads the **composited window output** — exactly what `naadf_final_blit_node` produced and
  what the user sees on a live run. The test asserts on the real on-screen frame.
- It is first-party and already wired for the async capture → `ScreenshotCaptured` handshake; a
  hand-rolled window-surface readback would re-implement `screenshot.rs`.

### 5.2 Frame-timing of the readback — the bounded `DRAIN` state

`Screenshot` capture is async (`screenshot.rs` doc: "may not be available immediately after the
frame that the component is spawned on"). The driver's `DRAIN` state (§4.1) ticks up to
`E2E_DRAIN_FRAMES` (= 4) extra frames waiting for the `ScreenshotCaptured` observer to populate
`E2eScreenshot`. As soon as it is populated the driver moves to `ASSERT`. If the drain bound is
exhausted with no screenshot, that itself is a failure: the driver writes `AppExit::error()`
with "no framebuffer produced — the render path never delivered a frame". The drain bound is
generous (4 frames) precisely so a slow-but-working readback is not a false failure.

### 5.3 Decoding the bytes

The window is created with a **plain, predictable surface format**. The blit pipeline
(`naadf_final.wgsl`) is specialised per the view target's main-texture format
(`prepare_blit_pipeline`, `graph.rs:195-199`); the captured `Image.texture_descriptor.format`
reports what it actually is (typically `Rgba8UnormSrgb` / `Bgra8UnormSrgb` for a window
surface). The `Framebuffer` wrapper (§6.2, §9) reads `Image.texture_descriptor.size` for the
dimensions and `Image.data` for the bytes, normalises the channel order from the reported
format (handle both `Rgba8*` and `Bgra8*` — a window surface is commonly BGRA), and exposes a
uniform `&[[u8; 4]]` indexed by `y * width + x`. **The impl agent must not assume RGBA** — it
must branch on `texture_descriptor.format`. (This is the one real complication the windowed
mechanism adds over the v1 offscreen `Image`, where the format was chosen by the test; here the
window surface format is the platform's choice and must be read, not assumed.)

---

## 6. Assertion strategy — region/statistic gates, not golden images

### 6.1 Decision: region/statistic assertions (with an *optional* golden-hash tripwire)

**Primary: robust region/statistic assertions.** Per the brief, lean away from brittle exact
golden images. Although §4 makes the output bit-identical *run-to-run on the same binary*, a
golden PNG is brittle across the dimension that matters here: **every batch deliberately changes
the image** (Batch 2 changes the sky, Batch 5 adds GI bounce, Batch 6 adds TAA), and several
"no visible change" batches (B3, B4) still touch buffers. A golden image would have to be
re-blessed on almost every batch, and a re-blessed golden hides regressions instead of catching
them. Region/statistic gates encode *what each batch is supposed to make true* — they are the
manual visual gates, mechanised.

**Secondary tripwire (optional, recommended): a stability hash.** For the batches that are
*supposed* to leave the image unchanged (B3, B4 — see brief item 6), the harness also stores a
**hash of the readback buffer** and asserts it equals the prior batch's stored hash. This is
the "output image stable vs. the prior baseline" gate. The hash baseline lives in a small
committed file (`src/e2e/baselines/<batch>.hash`) or a `const` table in the e2e module, and is
updated *only* by the batch that intentionally changes the image. Cheap, catches accidental
drift, not brittle because it is only asserted-equal where the design says "unchanged".

### 6.2 The per-batch gate functions

Each batch's visual gate is one function `assert_batch_N(fb: &Framebuffer) -> Result<(), String>`
where `Framebuffer` is a thin wrapper over the normalised `&[[u8;4]]` + dimensions (§5.3), with
helpers: `region_mean(rect) -> [f32;4]`, `pixel(x,y)`, `fraction_brighter_than(rect, thresh)`,
`is_near(color_a, color_b, tol)`, `luminance`. The test scene (`GridPreset::Default`) has a
known layout — a ground slab, axis-aligned boxes, a sphere, and **one emissive box**
(`voxel/grid.rs` doc comment + `build_default_volume`). The camera pose is fixed (§4.2), so each
feature occupies a **known screen rectangle**. The gates:

- **Batch 2 gate (4-plane first-hit + atmosphere) — the manual "emissive white / others black /
  sky" gate, mechanised:**
  - The emissive-block screen region: `region_mean` is **near-white / high-luminance** (the
    emissive material is the only lit thing pre-GI).
  - A non-emissive solid block region: **near-black** (no bounce light yet — Phase B pre-GI;
    the `base/` first-hit gives non-emissive diffuse surfaces no direct light until GI).
  - The sky region (a screen corner that misses all geometry): **sky-colored** — not black, not
    white; within a broad tolerance of the expected atmosphere tint, luminance in a mid band.
- **Batch 3, Batch 4 gates (no visible change):** `assert_batch_3` / `assert_batch_4` re-run the
  Batch-2 region gates **and** assert the §6.1 stability hash equals the Batch-2 baseline. Plus
  the §3 pipeline-error scan (which now also covers the B3/B4 pipelines) and the §8 node-dispatch
  check. This is exactly the brief's "boots, all expected render-graph nodes dispatch, no errors,
  output image stable vs. the prior baseline."
- **Batch 5 gate (GI bounce becomes visible):** the non-emissive block region that was
  near-black in Batch 2/3/4 is now **measurably brighter than the Batch-2 baseline** (bounce
  light from the emissive block has arrived) — assert `region_mean luminance > batch2_baseline
  luminance + margin`. The emissive region stays bright; the sky stays sky. The stability hash
  is **re-blessed** here (the image legitimately changed).
- **Batch 6 gate (TAA):** the image is temporally stable — capture the readback at two
  consecutive frames near the end of the `RUN` window (two `Screenshot` shots a frame apart, or
  one shot + one extra frame + a second shot) and assert the **per-pixel delta between
  consecutive frames is small** (TAA has converged — no per-frame shimmer). Also assert the
  image still passes the Batch-5 brightness gates (the blit source is back on
  `taa_sample_accum`). Re-bless the stability hash.

### 6.3 Tolerances

All gates use **generous tolerances** (region means, luminance bands, fractions-of-pixels —
not exact pixel equality). The point is to catch "the emissive block went black" or "GI never
turned on" or "the sky is now solid magenta because a bind group is mis-wired" — gross,
batch-level regressions — not sub-percent shading drift. Exact-pixel comparison is reserved for
the optional stability hash, and only where the design says the image must not change.

### 6.4 Extensibility — adding a batch is a small, obvious edit

A new batch adds:
1. one `assert_batch_N(&Framebuffer) -> Result<(), String>` function (a handful of region
   asserts),
2. one line in the dispatch table mapping the current batch number → its assert fn,
3. if the batch changes the image, one re-blessed hash baseline entry.

The window-boot, bounded-frame driver, screenshot readback, and pipeline-error-scan code is
**batch-agnostic and written once**. The "current batch" is a single `const CURRENT_BATCH: u32`
the impl agent bumps when a batch lands. The `ASSERT` step runs **the pipeline-error scan +
node-dispatch check unconditionally** (every batch benefits) and **the highest batch's region
gate**; older batches' region gates are kept as called helpers so a regression in an earlier
gate still trips.

### 6.5 Where the gates run

The `ASSERT` state-step (§4.1) runs in a main-world `Update` system. It:
- reads `E2eScreenshot` → builds a `Framebuffer` (§5.3),
- runs `is_degenerate` (§7), the current batch's `assert_batch_N`, and — for the consecutive-frame
  Batch-6 gate — the stored prior-frame `Framebuffer`,
- the **pipeline-error scan** (§3.1) and the **node-dispatch check** (§8) can run here too, by
  taking `&World` and reaching the `RenderApp` sub-app — *or*, simpler, the driver writes
  `AppExit` and `run_e2e_render` does the pipeline-scan + degenerate check after `app.run()`
  returns (the §11 plan splits it this way). Either is fine; the split keeps each piece in the
  place with the least plumbing.
- on any gate failure: `eprintln!` the failure detail and write `AppExit::error()`; on all-pass:
  write `AppExit::Success`.

The §6.2 known-rectangle constants are derived from the fixed camera pose + the
`GridPreset::Default` scene **at the actual physical readback resolution** — the impl agent
derives them once from a debug dump (write the first readback `Image` to a PNG via
`save_to_disk`, eyeball the rects) rather than guessing, and stores them as rects in fractional
(0..1) screen coords or in physical pixels keyed off `fb.width()/fb.height()` so a scale-factor
difference does not silently misalign them.

---

## 7. Readback sanity floor (degenerate-frame guard)

Independent of the per-batch gates, every run asserts the readback is **not degenerate**:
not all-identical-pixels (a stuck clear color), not all-zero (nothing rendered), and contains
both some dark and some bright pixels (geometry + sky present). This catches the "pipeline
silently `return`ed so the frame is the clear color" failure mode (§3.1) even before the
per-batch gate runs, and gives a clearer message ("framebuffer is uniformly black — the render
graph produced no output" vs. a confusing region-mean assertion). On failure: `eprintln!` +
`AppExit::error()`.

---

## 8. Render-graph node-dispatch check

The brief requires that batches with no visible change still assert "all expected render-graph
nodes dispatch." Mechanism: the nodes already wrap their work in a `time_span(encoder, SPAN)`
(`graph.rs:88`, and every `graph_b.rs` node has a `*_SPAN` const). `RenderDiagnosticsPlugin`
surfaces each as a `render/<span>/elapsed_cpu` (and `_gpu`) diagnostic (`hud.rs:14-29` documents
the path scheme). The harness:

- keeps `RenderDiagnosticsPlugin` + `FrameTimeDiagnosticsPlugin` in the e2e app (§2.2),
- in the `ASSERT` step (or in `run_e2e_render` post-`run()`), reads the `DiagnosticsStore` and
  asserts that **every expected span for the current batch has a recorded measurement** — i.e.
  the node actually ran (a node that early-returns because its pipeline failed records *no*
  span).

The "expected spans for batch N" is a small `const &[&str]` table next to the per-batch assert
functions — extends exactly like §6.4. This is a second, cheaper signal that complements the
§3.1 pipeline-error scan: §3.1 says "the pipeline is broken," the dispatch check says "the node
that uses it never ran."

---

## 9. Where the code lives + file structure

The e2e run is a **binary target**, not an integration test (§2.1 — the winit main-thread
constraint). It needs the real app wiring, so the binary crate must expose `build_app` as a
library surface.

```
src/
  lib.rs            re-exports the modules + `pub fn build_app(cfg: AppConfig) -> App`
                    + `pub fn run_e2e_render() -> AppExit`.
  main.rs           thin: `fn main() -> AppExit { bevy_naadf::build_app(AppConfig::windowed()).run() }`.
  bin/
    e2e_render.rs   `fn main() -> AppExit { bevy_naadf::run_e2e_render() }` — that's the whole file.
  e2e/
    mod.rs          declared `pub mod e2e;` from lib.rs. The e2e module:
    driver.rs       the §4.1 bounded-frame driver system + the E2e state-machine resource,
                    the AppExit-writing logic.
    readback.rs     the `Screenshot::primary_window()` spawn, the `stash_screenshot` observer,
                    the `E2eScreenshot` resource, the bounded DRAIN logic.
    checks.rs       `scan_pipeline_errors(&App)`, `assert_nodes_dispatched(&App, &[&str])`,
                    the degenerate-frame floor check — all batch-agnostic.
    framebuffer.rs  `Framebuffer` wrapper: from_image (format-aware §5.3), region_mean, pixel,
                    luminance, is_near, fraction_brighter_than, stability_hash, is_degenerate.
    gates.rs        assert_batch_2 .. assert_batch_N, the EXPECTED_SPANS tables, the
                    hash-baseline table, CURRENT_BATCH, the known-rectangle consts + the fixed
                    E2E camera pose const.
    baselines/      (optional) committed stability-hash files, if not kept as consts.
```

`run_e2e_render()` (in `lib.rs` or `e2e/mod.rs`): `build_app(AppConfig::e2e())`, then
`let exit = app.run();`, then run the post-run checks (pipeline-error scan, node-dispatch,
degenerate floor — the parts that need the returned `App`) and **fold their result into the
returned `AppExit`** (if `app.run()` returned `Success` but a post-run check failed, return
`AppExit::error()`). The per-batch *region* gates run inside the app in the `ASSERT` step (§6.5),
because they need the screenshot resource the app produced; their failure is already folded into
the `AppExit` the driver wrote.

**The `lib.rs` extraction is a prerequisite (§11 step 1).** Today `main.rs` builds the app
inside `fn main()`; neither `e2e_render.rs` nor `run_e2e_render` can call `fn main()`. Add a
`src/lib.rs` that re-exports the existing modules (`aadf`, `camera`, `hud`, `render`, `voxel`,
`world`, `AppArgs`, `GiSettings`, `GridPreset`) plus the new `e2e` module, and a
`pub fn build_app(cfg: AppConfig) -> App` carrying the plugin/system wiring currently in
`main.rs:102-161`, parameterised by an `AppConfig`:

```text
AppConfig {
    add_hud: bool,
    add_free_camera: bool,
    synchronous_pipeline_compilation: bool,
    window: WindowConfig,           // size, resizable, title
    add_e2e_systems: bool,          // the §4 driver + §6 ASSERT systems + the WinitSettings::game() insert
}
AppConfig::windowed()  -> the production config (HUD on, free camera on, async compile, default window)
AppConfig::e2e()       -> HUD off, free camera off, sync compile, 256×256 non-resizable window, e2e systems on
```

`main.rs` becomes `fn main() -> AppExit { bevy_naadf::build_app(AppConfig::windowed()).run() }`.
This is the clean idiomatic Rust layout (a `lib.rs` + thin `main.rs` + a thin `bin/`) and keeps
the e2e binary from duplicating ~60 lines of plugin wiring that would otherwise silently drift
from `main.rs`. **Rejected alternative:** duplicate the wiring inside `e2e_render.rs` — it
duplicates `main.rs` and *will* drift; the e2e run would stop testing the real app.

---

## 10. Methodology change — this REPLACES the live smoke-run as the agent's gate

**Binding process change, stated explicitly for every future Phase-B (and later) impl agent:**

- **Before:** impl-agent verification = `cargo build` + `cargo test` + an *open-ended* live
  `cargo run` smoke-run (open a window, watch for panics/validation errors/TDR for ~30s, kill
  it). The live run was the *only* thing that exercised WGSL/pipeline creation, and it cost N+
  slow window-opening reruns to clear N shader bugs (`10-impl-b.md` Batch 1).
- **After:** impl-agent verification = `cargo build` + `cargo test` (the existing pure-CPU unit
  suite) + **`cargo run --bin e2e_render`** (this bounded windowed e2e run, **once**). The e2e
  run exercises real WGSL composition, real pipeline creation, the real render graph, the real
  on-screen window, and the real framebuffer — in one bounded shot that exits with a process
  code, the naga/wgpu error text on stderr. A window appears for under a second and closes
  itself; the agent reads the exit code, it does **not** re-run in a loop.
- **The thing that changed is the *boundedness*, not the window.** A real window still appears —
  that is deliberate and correct (the user rejected headless). What is gone is the open-ended
  "run, watch, kill, repeat" loop: the run now terminates itself deterministically after a
  fixed frame count and reports pass/fail as the exit code.
- **The live free-fly `cargo run --bin bevy-naadf` stays the *user's* subjective review-gate
  check** — the per-batch "user interactive re-test confirms GI is rendering / temporally
  stable / no artifacts" gate (`01-context.md` §2c, §2d done-bars). It is no longer part of the
  *agent's* loop. The agent ships when `cargo build` + `cargo test` + `cargo run --bin
  e2e_render` are all green; the user then does the subjective visual pass.
- This also fixes the `subagent-gpu-app-verification-loop` memory hazard: the agent no longer
  rebuilds→reruns an open-ended windowed app chasing a visual outcome — it has a deterministic,
  one-shot, self-terminating run instead.

The impl agent that builds this harness must add a note to `10-impl-b.md` (and the orchestrator
should reflect it in the README checklist) recording the methodology change so subsequent batch
agents follow it.

---

## 11. Implementation plan (ordered — for the follow-up impl agent)

Each step ends compiling. Steps 1–7 build the batch-agnostic harness against the **current**
tree state (Batches 1–3 implemented); step 8 adds the gates for the batches that exist now.

1. **Extract `src/lib.rs` + `AppConfig`** (§9). Create `src/lib.rs` re-exporting the existing
   modules + the new `e2e` module, and a `pub fn build_app(cfg: AppConfig) -> App` carrying the
   wiring from `main.rs:102-161`, parameterised (`add_hud`, `add_free_camera`,
   `synchronous_pipeline_compilation`, `window`, `add_e2e_systems`). Add `AppConfig::windowed()`
   + `AppConfig::e2e()`. Rewrite `main.rs` to
   `fn main() -> AppExit { bevy_naadf::build_app(AppConfig::windowed()).run() }`. `cargo build`
   + the existing `cargo test --bin bevy-naadf` stay green (pure refactor, no behaviour change).
2. **`src/bin/e2e_render.rs`** — the whole file is
   `fn main() -> AppExit { bevy_naadf::run_e2e_render() }`. (Cargo auto-discovers it as a binary
   target; no `Cargo.toml` change needed. No feature gate — the e2e binary always builds and
   always runs, GPU required, §2.3.)
3. **`src/e2e/readback.rs`** — `E2eScreenshot(Option<Image>)` resource, the `stash_screenshot`
   observer (`On<ScreenshotCaptured>` → store `image` into the resource), the helper that
   spawns `Screenshot::primary_window()` with the observer attached.
4. **`src/e2e/driver.rs`** — the §4.1 bounded-frame state machine: an `E2eState` resource
   (`Run/Shoot/Drain/Assert` + frame counter), the `e2e_driver` `Update` system that advances
   it, spawns the screenshot at `Shoot`, waits in `Drain`, and at `Assert` builds the
   `Framebuffer`, runs the current batch's gates (§6.5) + `is_degenerate`, and writes
   `AppExit::Success` / `AppExit::error()` accordingly. Also inserts
   `WinitSettings { focused_mode: Continuous, unfocused_mode: Continuous }` (§2.2) via the e2e
   config.
5. **`src/e2e/framebuffer.rs`** — `Framebuffer { data: Vec<[u8;4]>, w, h }` + `from_image`
   (format-aware: branch on `Image.texture_descriptor.format`, normalise RGBA/BGRA — §5.3),
   `region_mean`, `pixel`, `luminance`, `is_near`, `fraction_brighter_than`, `stability_hash`
   (a stable hash, e.g. `DefaultHasher` over the bytes), `is_degenerate` (§7).
6. **`src/e2e/checks.rs`** — `scan_pipeline_errors(&App) -> Result<(), String>` (reach into
   `RenderApp` via `app.sub_apps()`, get `PipelineCache`, iterate `.pipelines()`, collect every
   `CachedPipelineState::Err` with descriptor label + `ShaderCacheError` message);
   `assert_nodes_dispatched(&App, &[&str]) -> Result<(), String>` (read `DiagnosticsStore`,
   assert each expected `render/<span>/elapsed_cpu` has a measurement).
7. **`src/e2e/mod.rs` + `run_e2e_render`** — `pub fn run_e2e_render() -> AppExit`:
   `let mut app = build_app(AppConfig::e2e());`, spawn the fixed-pose camera + the 256×256
   window are part of `AppConfig::e2e()`'s startup systems; `let exit = app.run();` (the winit
   runner drives it, the driver self-terminates); then **unconditionally** run
   `scan_pipeline_errors(&app)` + `assert_nodes_dispatched(&app, current_batch_spans)` and fold
   any failure into the returned `AppExit` (return `AppExit::error()` if `exit` was `Success`
   but a post-run check failed; otherwise return `exit`).
8. **`src/e2e/gates.rs` — gates for the implemented batches** — the fixed E2E camera-pose const,
   the `Framebuffer` known-rectangle constants for the `GridPreset::Default` scene at that pose
   (emissive-block rect, solid-block rect, sky rect — derived once by the impl agent from a
   `save_to_disk` PNG dump of the first readback), `assert_batch_2` (emissive-white/solid-black/
   sky), `assert_batch_3` + `assert_batch_4` (= Batch-2 gates + stability hash), the
   `EXPECTED_SPANS` table for the Batches 1–3 node set (`naadf_atmosphere`, `naadf_first_hit`,
   `naadf_ray_queue`, `naadf_global_illum`, `naadf_final_blit`), the hash-baseline table, and
   `CURRENT_BATCH = 3`. Bless the initial stability hash from a first green run.
9. **Verify** — `cargo run --bin e2e_render` exits 0 on the dev box (window opens for <1 s,
   closes itself); `cargo build` + `cargo test --bin bevy-naadf` stay green. Add the §10
   methodology note to `10-impl-b.md` and flag it to the orchestrator for the README checklist.
10. **Per-batch upkeep (not this agent — the rule for future batch agents)** — each subsequent
    batch (B4 onward, B5, B6) adds its `assert_batch_N`, its `EXPECTED_SPANS` row, bumps
    `CURRENT_BATCH`, and re-blesses the stability hash *iff* the batch intentionally changes the
    image (B5, B6 do; B4 does not). This is the §6.4 small-obvious-edit.

---

## 12. Open questions & risks

- **R2 — `ScreenshotCaptured` latency.** The async window-surface capture may take more than one
  frame to deliver. §5.2's bounded `DRAIN` state (4 frames) handles it; if the screenshot never
  arrives within the bound the run fails with "no framebuffer produced" — which is *correct*
  (the render path is broken) but the impl agent should keep the drain bound generous (4
  frames), not tight. Low risk.
- **R3 — Pipeline-creation is lazy.** §3.3: a pipeline is only queued once its node runs with
  all upstream resources present. The 8-render-frame budget is an empirical estimate
  (resource-build latency is ~3 frames). If a future batch adds a pipeline gated on a runtime
  condition (e.g. the denoiser node gated on `is_denoise`), the e2e scene must hold that
  condition true — `GiSettings::default()` has all bools `true` (`main.rs:81-86`), so the
  default config already exercises every conditional Phase-B node. Worth a re-check when B5's
  denoiser lands.
- **R4 — Layer-B device-error capture is best-effort.** Bevy's `RenderDevice` wrapper does not
  expose a wgpu uncaptured-error scope to test code (verified — no such API on the 0.19-rc.1
  wrapper). The harness relies on panic-propagation (a panic in a windowed app aborts the
  process non-zero) + the readback content gates (§3.2). This caught Batch 1 bug #3 (it TDR'd →
  `DeviceLost` panic) in practice, but a *silent* validation error that neither panics nor
  visibly corrupts the frame would slip past Layer B. Layer A (the pipeline-error scan) is
  unaffected and remains the load-bearing check. Best-effort Layer B is judged acceptable — a
  custom `device.poll(Maintain::Wait)` node would be more invasive and is not worth it.
- **R5 — Known-rectangle constants are camera-pose-coupled.** The §6.2 region rectangles are
  derived from the fixed camera pose + the `GridPreset::Default` scene. If the e2e camera pose
  or the test grid changes, the rectangles must be re-derived. Mitigation: keep the e2e camera
  pose a single named const in `gates.rs`, document that the rectangles are derived from it,
  store them in fractional screen coords (or keyed off `fb.width()/height()`), and have the
  impl agent derive them from an actual `save_to_disk` PNG dump rather than guessing. Not a
  blocker — a maintenance note.
- **R7 (new — windowed-specific) — window surface format is platform-chosen.** Unlike the v1
  offscreen `Image` (where the test picked the format), the real window surface format is the
  platform/driver's choice — commonly `Bgra8UnormSrgb` on Vulkan/Linux, not `Rgba8UnormSrgb`.
  `Framebuffer::from_image` **must** branch on `Image.texture_descriptor.format` and normalise
  channel order (§5.3), not assume RGBA. This is a correctness requirement on the impl, called
  out so it is not missed — it is the one genuine new complication the windowed mechanism adds.
  Low risk if honoured, a silent channel-swap bug if not. (On a HiDPI desktop the physical
  window size may also differ from the requested logical size by the scale factor — §4.2 / §6.5
  handle this by deriving rects from the actual readback dimensions.)
- **R8 (new — windowed-specific) — desktop window-manager interaction.** A real on-screen window
  on a busy desktop can momentarily lose focus or be obscured. `UpdateMode::Continuous` in both
  focused and unfocused modes (§2.2) means the app keeps ticking regardless, so the bounded
  frame loop still advances and the run still terminates — focus loss does not stall it. The
  window is only up for <1 s. Judged a non-issue given the `Continuous` setting; noted only so
  a future reader knows it was considered, not overlooked.

(The v1 design's **R1 — GPU-less environments** is **removed**: there is a GPU on the only
machine this runs on and no CI; the e2e binary unconditionally requires a GPU and always runs —
§2.3. The v1 **R6 — `synchronous_pipeline_compilation` on the test app only** is folded into
§2.2 as a settled design point, not an open risk: `AppConfig` parameterises it, `windowed()`
keeps async, `e2e()` flips it on.)

---

## Revision (2026-05-14): windowed mechanism, GPU-less concern dropped

This document was revised in place from a v1 that chose a **headless** mechanism. Two user
corrections drove the revision:

1. **GPU-less / CI / feature-gate concern dropped entirely.** v1's "R1" flagged GPU-less-environment
   handling and sketched an `e2e-gpu` Cargo feature gate + a runtime self-skip. All of it is
   gone: there is a GPU on the only machine this runs on and no CI to accommodate. The e2e
   binary unconditionally requires a GPU and always runs (§2.3). No feature, no `#[ignore]`, no
   adapter-detection branch. Deleted from §2.3, the §11 plan, and the §12 risk list.
2. **Windowed, not headless.** v1 chose `DefaultPlugins.disable::<WinitPlugin>()` +
   `ScheduleRunnerPlugin::run_once()` + a manual `app.update()` loop + an offscreen
   `RenderTarget::Image` + `GpuReadbackPlugin`. The user rejected headless. The mechanism is now:
   - A **dedicated binary target** `src/bin/e2e_render.rs` (`fn main() -> AppExit`), invoked
     **`cargo run --bin e2e_render`** — *not* a `#[test]` (winit needs the event loop on the
     main thread; `cargo test` runs tests on worker threads — §2.1).
   - The **real `DefaultPlugins` + `WinitPlugin`** — a real on-screen window, the same wiring as
     `main.rs`, four minimal deliberate deltas via `AppConfig::e2e()` (§2.2).
   - A **bounded-frame driver system** (§4.1) instead of a manual `update()` loop: it counts a
     fixed number of frames, then writes `AppExit`; the **winit runner** sees `should_exit()` and
     exits the event loop, returning the `AppExit` (§2.4). The window appears for <1 s and
     closes itself.
   - Framebuffer readback via **`Screenshot::primary_window()` + `ScreenshotCaptured` observer**
     (§5) — the first-party API for reading back a *real window* surface, replacing the
     headless-only `GpuReadbackPlugin` + offscreen `Image`.
   - **Single deterministic invocation preserved** — that property was always the point. The
     thing the user rejected was the open-ended "run / watch / kill / repeat" loop, not a window
     appearing. The run is still bounded, deterministic, and self-terminating with a process
     exit code.

Everything else from v1 survives, adjusted for the windowed mechanism: the load-bearing
`PipelineCache` `CachedPipelineState::Err` scan (§3.1), the region/statistic per-batch gates +
optional stability-hash tripwire (§6), the determinism strategy (§4.2), the degenerate-frame
floor (§7), the node-dispatch check (§8), per-batch extensibility (§6.4), the `src/lib.rs` /
`build_app` extraction prerequisite (§9, §11 step 1), and the §10 methodology change (this
replaces the live smoke-run as the *agent's* gate; the agent runs it once, not in a loop).

---

## Decisions & rejected alternatives

- **Dedicated binary `src/bin/e2e_render.rs`, not a `#[test]` or `cargo run --example`.**
  Rejected `#[test]`: winit requires the event loop on the main thread and `cargo test` runs
  test fns on worker threads — a real window cannot be created there (this is *the* constraint
  the brief named). Rejected `cargo run --example`: an example would also work mechanically, but
  examples conventionally live in `examples/` and are demo code; a verification entry point that
  impl agents run as a gate reads more correctly as a `bin/` target, and `src/bin/*.rs` is
  auto-discovered with zero `Cargo.toml` ceremony. *Flips if:* Bevy ever ships a way to run
  winit off the main thread, or the project adopts an `examples/`-based convention for tooling.
- **`fn main() -> AppExit`, process exit code = test result, no glue.** Chose returning the
  `AppExit` directly because `AppExit: Termination` (`app.rs:1594`) makes the kernel exit code
  fall out for free — 0 on `Success`, non-zero on `Error`. Rejected wrapping `app.run()` in
  manual `std::process::exit(code)` calls: redundant, and `process::exit` skips destructors.
  *Flips if:* the run needs to emit a structured report file before exit — then a small explicit
  exit-code path might be cleaner, but even then `Termination` still works.
- **Bounded-frame *driver system* + winit runner's `should_exit()`, not a manual `app.update()`
  loop.** v1 (headless) called `app.update()` in a `for` loop because it had no runner. With the
  real `WinitPlugin` the runner owns the loop, so the bounded behaviour is expressed as a system
  that writes `AppExit` after N frames — the runner's existing `should_exit()` check
  (`state.rs:735`) then terminates it. Rejected: trying to call `app.update()` manually *and*
  keep `WinitPlugin` — they fight over the loop. *Flips if:* a future need requires driving
  frames from outside Bevy, which would itself conflict with the windowed requirement.
- **`Screenshot::primary_window()` for readback, not `GpuReadbackPlugin`.** v1 used
  `GpuReadbackPlugin` + `Readback::texture(image)` — but that only worked because v1's camera
  rendered to an offscreen `Image` (the rejected headless mechanism). A real window's swapchain
  surface texture is not directly `COPY_SRC`-mappable; `Readback::texture` wants an `Image`
  handle. `Screenshot::primary_window()` (`screenshot.rs:98`) is the API Bevy provides
  *specifically* to read back a window and owns the surface→`COPY_SRC`→`MAP_READ` handshake.
  Rejected the alternative the brief offered (render the camera additionally to an `Image`
  *while also* showing a real window): it doubles the render cost, needs a second camera or a
  second target, and `Screenshot` already reads the exact composited window output the user
  sees — strictly simpler and more faithful. *Flips if:* a batch needs readback of an
  intermediate buffer (not the final window) — then a targeted `GpuReadbackPlugin` on that
  specific `Image`/buffer would be added *alongside* the window screenshot, not instead of it.
- **`WinitSettings::game()` (`Continuous` focused + unfocused).** Chose `Continuous` in both
  modes because the default `WinitSettings` drops to `reactive_low_power` when unfocused
  (`winit_config.rs:21`), and a `Reactive` mode only ticks on input events — the bounded frame
  driver would stall if the e2e window never gains focus on a busy desktop. Rejected leaving the
  default: it makes the run length depend on window-manager focus behaviour, i.e. non-deterministic.
  *Flips if:* nothing realistic — `Continuous` is unconditionally correct for a self-driving
  bounded run.
- **256×256 fixed, non-resizable window.** Chose small + fixed so the readback is fast, the GI
  dispatch is cheap, and every `pixel_count`-sized buffer is identical run-to-run. Rejected a
  larger or default-sized window: slower, and on a HiDPI desktop a default logical size scales
  unpredictably to physical pixels. *Flips if:* a gate needs more spatial resolution to be
  stable — bump the const, re-derive the §6.2 rects.
- **`build_app(AppConfig)` extraction into `src/lib.rs`, production `main.rs` goes thin.**
  Chosen so the e2e binary calls the *real* app wiring; rejected duplicating the wiring in
  `e2e_render.rs` because it would drift from `main.rs` and the e2e run would stop testing the
  real app. (Unchanged from v1 — still correct for the windowed mechanism; the e2e binary needs
  the library surface just as the headless test did.) *Flips if:* never, within this project's
  single-crate constraint (Q4).
- **Region/statistic gates primary, stability-hash tripwire secondary.** Unchanged from v1 and
  reaffirmed: every batch deliberately changes the image, so a golden image would be re-blessed
  almost every batch and would hide regressions; region gates encode *what each batch makes
  true*. The hash is asserted-equal only where the design says "image unchanged" (B3, B4).
  *Flips if:* a batch's output becomes genuinely impossible to characterise with region
  statistics — then a scoped golden for that region, re-blessed deliberately, is the fallback.
- **Layer A (pipeline-error scan) load-bearing, Layer B (panic + content) best-effort.**
  Unchanged from v1. The pipeline-error scan catches the entire `10-impl-b.md` shader-bug
  catalogue at its terminal `Err` state; device-error capture has no clean Bevy API so it leans
  on panic-propagation (which in a windowed binary aborts the process non-zero) + the content
  gates. *Flips if:* Bevy exposes a wgpu uncaptured-error scope on `RenderDevice` — then Layer B
  becomes a real check.

## Assumptions made

- **The fixed E2E camera pose frames all the §6.2 gate features.** Assumed the `setup_camera`
  pose (`camera/mod.rs:40`) or a near variant places the emissive block, a non-emissive solid
  block, and a sky corner in non-overlapping known screen rectangles. The impl agent derives the
  exact rects from a `save_to_disk` PNG dump of the first readback (§6.5, §11 step 8); if no
  single pose frames all three cleanly, a test-specific pose const is chosen — the design already
  allows this (§4.2). Not a blocker, but if the `GridPreset::Default` layout makes some feature
  un-frameable the gate set for that feature shrinks.
- **`GridPreset::Default` has exactly one emissive block and is RNG-free.** Carried from v1's
  read of `voxel/grid.rs`; the impl agent should re-confirm the emissive-block count and the
  deterministic construction when deriving the rects. If the scene has multiple emissive blocks
  or any RNG, §4.2's "test scene" row and §6.2's "emissive region" gate need adjusting.
- **The window surface format on the dev box is one of `Rgba8*` / `Bgra8*` 8-bit-4-channel.**
  `Framebuffer::from_image` (§5.3, R7) is specified to branch on
  `Image.texture_descriptor.format` and normalise — assumed the platform picks a plain 8-bit
  RGBA/BGRA surface (the overwhelmingly common case on Vulkan/Linux). If the surface is some
  exotic format (e.g. 10-bit), `from_image` needs another branch; low likelihood.
- **8 render frames + 4 drain frames is enough for the current Phase-B pipeline set.** Carried
  from v1's empirical estimate (resource-build latency ~3 frames; `synchronous_pipeline_compilation`
  resolves pipelines the frame they are queued). Assumed no current node needs more than ~3
  frames of resource warm-up. R3 flags the re-check point (B5's conditional denoiser node). If a
  pipeline is still `Queued` at the scan, the fix is bumping `E2E_RENDER_FRAMES`, not a redesign.
- **`save_to_disk` is available for the one-time rect-derivation dump.** Assumed the impl agent
  can use `Screenshot` + `save_to_disk` (`screenshot.rs:134`) once during development to dump a
  PNG and eyeball the gate rectangles. This is a dev-time aid, not part of the shipped e2e run.
- **The e2e binary runs in the worktree root** (so `AssetPlugin { file_path: "src/assets" }`
  resolves) — same working-directory assumption `main.rs` already makes. `cargo run --bin
  e2e_render` from the worktree root satisfies this.
- **A panic inside `app.update()` in a windowed app aborts the process with a non-zero code.**
  Assumed the winit runner does not catch-unwind around the app update (consistent with v1's
  Layer-B reasoning and with Batch 1 bug #3 manifesting as a hard `DeviceLost` abort). If the
  runner ever swallowed panics, Layer B's panic-propagation signal would weaken — but Layer A
  (the pipeline scan) is unaffected and remains load-bearing.

---

## Implementation log (2026-05-14)

Implements the §11 10-step plan. Worktree
`/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/phase-b-gi`, branch
`feat/phase-b-gi`. Result: `cargo build` clean, `cargo test` 44 pass (unchanged),
`cargo run --bin e2e_render` exits 0 with B1–B3 gates green.

### Files created / changed

**Created:**

| file | what it does |
|---|---|
| `src/lib.rs` | The shared library surface. Re-exports the existing modules (`aadf`, `camera`, `hud`, `render`, `voxel`, `world`) + the new `e2e` module, hoists `AppArgs` / `GiSettings` / `GridPreset` out of `main.rs`, and carries `pub fn build_app(cfg: AppConfig) -> App` — the single app-wiring path both binaries share. Adds `AppConfig` + `WindowConfig` (the four deliberate e2e deltas) with `AppConfig::windowed()` / `AppConfig::e2e()`, and `pub fn run_e2e_render() -> AppExit` delegating to `e2e::run_e2e_render`. |
| `src/bin/e2e_render.rs` | The whole binary: `fn main() -> AppExit { bevy_naadf::run_e2e_render() }`. |
| `src/e2e/mod.rs` | The harness module root. Frame-budget consts (`E2E_WIDTH/HEIGHT = 256`, `E2E_RENDER_FRAMES = 8`, `E2E_DRAIN_FRAMES = 8`), `add_e2e_systems` (inserts `WinitSettings` `Continuous`-both-modes, the e2e resources, the fixed-pose camera spawn, the `Update` driver, and — into the `RenderApp` — the render-world `PipelineCache` scan system), `setup_e2e_camera` (the production camera component set minus `FreeCamera`, at the fixed pose), and `run_e2e_render` (builds `AppConfig::e2e()`, runs it; the driver does every check inside the app). |
| `src/e2e/driver.rs` | The bounded-frame state machine: `E2ePhase` (`Run/Shoot/Drain/Assert/Done`), `E2eState`, `E2eOutcome`, and the `e2e_driver` `Update` system. At `ASSERT` it runs `run_assertions` — degenerate-frame floor + per-batch region gate + node-dispatch check + `PipelineCache` scan — folds *all* failures into one message, and writes `AppExit::Success`/`AppExit::error()`. |
| `src/e2e/readback.rs` | `E2eScreenshot(Option<Image>)` resource, the `stash_screenshot` observer (`On<ScreenshotCaptured>` → stash the `Image`), and `shoot_primary_window` (spawns `Screenshot::primary_window()` + the observer). |
| `src/e2e/framebuffer.rs` | `Framebuffer` (format-normalised RGBA `u8` grid) + `Rect` (fractional-coord rects keyed off the actual readback size). `from_image` branches on `texture_descriptor.format` (R7). Helpers: `region_mean`, `pixel`, `luminance`, `region_luminance`, `fraction_brighter_than`, `is_near`, `mean_pixel_delta` (the B6 temporal metric), `stability_hash`, `check_not_degenerate` (the §7 floor). |
| `src/e2e/checks.rs` | `PipelineScanResult` (the cross-world `Arc<Mutex>` channel), `scan_pipeline_errors_render_system` (the render-world `PipelineCache` `Err`-state scan — the load-bearing §3.1 check), `pipeline_scan_result` (the main-world read), and `assert_nodes_dispatched` (the §8 node-dispatch check against the main-world `DiagnosticsStore`). |
| `src/e2e/gates.rs` | Everything camera-pose-coupled in one place (R5): `e2e_camera_transform` (the fixed pose), `CURRENT_BATCH = 3`, the three gate rects (`emissive_rect` / `solid_block_rect` / `sky_rect`, fractional coords), `assert_batch_2` / `assert_batch_3`, `expected_spans` / `batch_gate` / `batch_needs_second_frame` dispatch tables, and `hash_baseline`. |

**Changed:**

| file | change |
|---|---|
| `src/main.rs` | Reduced to a thin shim: `fn main() -> AppExit { build_app(AppConfig::windowed()).run() }`. All wiring moved to `src/lib.rs`. |
| `Cargo.toml` | Added explicit `[lib] name = "bevy_naadf"`, `[[bin]] bevy-naadf`, `[[bin]] e2e_render` (the e2e bin is also cargo-auto-discovered; listed for clarity). |

### How the `src/lib.rs` extraction was done

`main.rs`'s `mod` declarations + `AppArgs`/`GiSettings`/`GridPreset` definitions +
the `App::new()…add_plugins…add_systems` body moved verbatim into `src/lib.rs`,
re-homed under `pub fn build_app(cfg: AppConfig) -> App`. The wiring is
parameterised by `AppConfig` along exactly the four §2.2 deltas: `add_hud`,
`add_free_camera`, `synchronous_pipeline_compilation` (threaded into a
`DefaultPlugins.set(RenderPlugin { … })`), `window` (threaded into a
`DefaultPlugins.set(WindowPlugin { … })`), and `add_e2e_systems` (calls
`e2e::add_e2e_systems`). The production path is `AppConfig::windowed()` — HUD on,
free camera on, async compile, default window — so `cargo run` is behaviour-identical
to before (verified: 44 tests still pass, build clean). The crate is now a library
+ two thin binaries; the existing `#[cfg(test)]` unit tests now live in the lib
suite — `cargo test` runs them (the old `cargo test --bin bevy-naadf` would now
find 0 tests; the verification command is `cargo test`).

### How the five flagged open items were resolved

- **R2 — screenshot-readback latency.** The driver's `DRAIN` phase polls
  `E2eScreenshot` for up to `E2E_DRAIN_FRAMES = 8` extra frames (the design
  suggested 4; bumped to 8 for extra slack — it is pure margin, not cost) before
  declaring "no framebuffer produced". In practice the capture arrives within
  ~1–2 drain frames. Generous bound, no false failure.
- **R3 — frame budget sized for lazy pipeline creation.** `E2E_RENDER_FRAMES = 8`
  render frames precede the `SHOOT`. With `synchronous_pipeline_compilation: true`
  every pipeline a node queues resolves to `Ok`/`Err` the same frame, so 8 frames
  is comfortably past the ~3-frame resource-build latency — *and* the `PipelineCache`
  scan additionally fails on any still-`Queued`/`Creating` pipeline, so an
  under-budget run is caught loudly rather than passing blind. `GiSettings::default()`
  has every GI bool `true`, so the default e2e scene exercises every conditional
  Phase-B node. Confirmed: the run reports "every pipeline created cleanly".
- **R4 — device-error capture stays best-effort.** No Layer-B machinery was added
  beyond what the design specifies: a panic in `app.update()` (DeviceLost / failed
  submit) propagates through the winit runner and aborts the process non-zero, and
  the degenerate-frame floor catches a mis-fed-but-compiled pipeline as a content
  failure. The load-bearing catch is the `PipelineCache` `Err`-state scan (Layer A).
- **R5 — assertion rects coupled to the fixed camera pose.** Everything pose-coupled
  lives in `src/e2e/gates.rs`: the pose is a single named const `e2e_camera_transform`,
  the three rects are fractional-coord helpers right below it, and the module header
  documents that the rects are derived from that pose. The rects were derived from
  an actual `save_to_disk` PNG dump (the dev-time aid, since reverted) — *not*
  guessed. The production `setup_camera` pose framed empty space at the e2e
  256×256 1:1-aspect window, so a **test-specific pose** was chosen (the design
  explicitly allows this — §4.2 / Assumptions). Verified-by-dump region luminances:
  emissive ~188, solid-geometry ~3, sky ~39 — well-separated, generous gate margins.
- **R7 — readback branches on the platform surface format.** `Framebuffer::from_image`
  matches on `image.texture_descriptor.format`: `Rgba8Unorm`/`Rgba8UnormSrgb` →
  no swap, `Bgra8Unorm`/`Bgra8UnormSrgb` → swap R↔B to normalise to RGBA, any
  other format → a hard `Err` ("add a branch for this format") rather than a
  silent channel-swap. On the dev box (RTX 5080 / Vulkan) the surface is a Bgra8
  format and the swap path is exercised — the gates pass, confirming the
  normalisation is correct.
- **R8 — `Continuous` update mode.** `add_e2e_systems` inserts
  `WinitSettings { focused_mode: Continuous, unfocused_mode: Continuous }` so the
  app ticks every frame regardless of focus — the bounded frame loop advances and
  the run self-terminates even if the e2e window never gains focus.

### Invocation, results

- **Command:** `cargo run --bin e2e_render` (from the worktree root).
- **`cargo build`:** clean (pre-existing dead-code warnings only).
- **`cargo test`:** 44 passed (was 44 — the lib extraction is a pure refactor; no
  test added or removed). Note: tests now run in the lib suite, so the command is
  `cargo test`, not `cargo test --bin bevy-naadf`.
- **`cargo run --bin e2e_render`:** exits **0**. A 256×256 window opens for under
  a second and closes itself; stdout: `e2e_render: PASS (batch 3) — 8 render
  frames, framebuffer read back & non-degenerate, per-batch region gate green,
  every pipeline created cleanly, every expected render-graph node dispatched.`

### Live batch gates (B1–B3) + how a future batch adds its gate

`CURRENT_BATCH = 3`. The `ASSERT` step runs, unconditionally every run: the
degenerate-frame floor, the node-dispatch check (`EXPECTED_SPANS` for B1–B3 =
`naadf_atmosphere`, `naadf_first_hit`, `naadf_ray_queue`, `naadf_global_illum`,
`naadf_final_blit`), and the `PipelineCache` error scan. Plus the highest batch's
region gate:

- **B1** — no visible-change gate of its own (the atmosphere precompute writes a
  buffer the blit does not read); covered by the floor + pipeline scan + dispatch
  check.
- **B2** — `assert_batch_2`: emissive-block region near-white (luminance > 120),
  non-emissive solid-block region near-black (luminance < 90), sky region in the
  [10, 230] mid-band and brighter than the un-lit solid block.
- **B3** — `assert_batch_3`: re-runs the B2 region gate (Batch 3 leaves the image
  unchanged — it only writes GI buffers the blit does not read) + asserts the
  stability hash equals the B3 baseline *once one is blessed* (`hash_baseline(3)`
  is currently `None` — see "remaining issue").

**A future batch (B4/B5/B6) adds its gate with a small, obvious edit in
`src/e2e/gates.rs`:** (1) write `assert_batch_N(&GateState) -> Result<(), String>`,
(2) add its arm to `batch_gate` and its row to `expected_spans` (B4 adds the
`naadf_sample_refine*` spans, B5 the denoiser span, B6 the TAA-node spans), (3)
bump `CURRENT_BATCH`, (4) if the batch intentionally changes the image (B5's GI
bounce, B6's TAA) re-bless its `hash_baseline` entry. The window-boot, driver,
readback, pipeline scan, and node-dispatch check are batch-agnostic and untouched.

### Remaining issue (one)

The §6.1 **stability-hash baseline is not yet blessed** (`hash_baseline` returns
`None` for every batch). The harness landed *alongside* Batch 3, and the readback
is only bit-identical run-to-run *on the same binary* — a committed hash literal
would just be re-derived on each dev box. The first real baseline should be
blessed by **Batch 4** (the first "no visible change" batch to land after the
harness): capture the Batch-3 readback `stability_hash` and pin it as
`hash_baseline(4)`. Until then the B3/B4 "image unchanged" guard rests on the
re-run B2 region gate, which still catches gross regressions. This is a
deliberate deferral, not a defect — the region gates are the primary check; the
hash is the optional tripwire (§6.1).
