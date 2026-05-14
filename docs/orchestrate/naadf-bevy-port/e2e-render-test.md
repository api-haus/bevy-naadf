# e2e-render-test — headless end-to-end render test harness

## delegate-architect findings (2026-05-14)

A design for a **single-`cargo-test` headless end-to-end rendering test** that boots the real
Bevy app (real `RenderDevice`, real render graph, real WGSL pipeline creation) with no winit
window, runs the render graph for a fixed frame count, reads the result back to the CPU, and
asserts per-batch visual gates. It **replaces the live `cargo run` smoke-run as the impl
agent's verification step** (see §10).

Worktree: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/phase-b-gi`, branch `feat/phase-b-gi`.
Test command stays `cargo test --bin bevy-naadf` (binary-only crate — `10-impl-b.md`); this
harness adds an **integration test crate** under `tests/`, so the full command becomes
`cargo test` (runs both the existing `--bin` unit tests and the new `tests/` integration test).

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

This harness collapses that into one `cargo test`: it boots the app headless, forces **every**
Phase-B pipeline to be created, and fails the test — with the naga/wgpu error message — if any
pipeline lands in the error state. It is the impl agent's fast inner loop.

---

## 2. Headless boot mechanism (Bevy 0.19-rc.1 specifics)

### 2.1 The core difficulty

The render-graph nodes (`src/render/graph.rs`, `src/render/graph_b.rs`) draw into a
**`ViewTarget`** (`naadf_final_blit_node`, `graph.rs:182-233` — `view_target.main_texture_view()`).
A `ViewTarget` exists only for a camera whose `RenderTarget` is a real surface (a window) or an
**`Image`**. There is no window in a test. So the harness renders the camera to an
**off-screen `Image`** (`RenderTarget::Image`) — Bevy builds a full `ViewTarget` for an
`Image` target exactly as for a window (`bevy_render-0.19.0-rc.1/src/camera.rs:237,258` —
`NormalizedRenderTarget::Image` is a first-class target), so the entire `Core3d` graph,
including `naadf_final_blit_node`, runs unchanged.

### 2.2 Plugin set — `DefaultPlugins` minus `WinitPlugin`, plus `ScheduleRunnerPlugin`

`DefaultPlugins` is a `PluginGroup`; `WinitPlugin` is one member
(`bevy_internal-0.19.0-rc.1/src/default_plugins.rs:40`). The harness builds the app with:

```text
DefaultPlugins
  .set(AssetPlugin { file_path: "src/assets".into(), ..default() })   // same as main.rs:128-131
  .disable::<bevy::winit::WinitPlugin>()                              // no window, no event loop
  .set(ImagePlugin::default())                                         // (default; explicit for clarity)
.add(bevy::app::ScheduleRunnerPlugin::run_once())                      // we drive frames manually instead
```

- `WinitPlugin` is the *only* window/event-loop dependency in `DefaultPlugins`; disabling it
  leaves `RenderPlugin`, `AssetPlugin`, `bevy_render`, `Core3d`, the asset server, ECS, etc.
  fully intact. `RenderPlugin` still creates a real `RenderDevice`/`RenderQueue` from wgpu.
- `ScheduleRunnerPlugin::run_once()` replaces the winit runner so `app.run()` would tick once
  and return — but the harness does **not** call `app.run()`; it calls `app.update()` in a loop
  (§4). `app.update()` pumps all sub-apps including `RenderApp`
  (`bevy_app-0.19.0-rc.1/src/app.rs:161-166` → `self.sub_apps.update()`), which is what we need.
  Including `ScheduleRunnerPlugin` just keeps the plugin set internally consistent (no winit
  runner left dangling); the manual `update()` loop is the actual driver.
- **`RenderPlugin { synchronous_pipeline_compilation: true, .. }`** — set this. With it,
  `PipelineCache` blocks until each queued pipeline reaches `Ok` or `Err` within the same
  `app.update()` (`bevy_render-0.19.0-rc.1/src/lib.rs:129-133,463`). Without it, pipeline
  creation is a background `Task` and "did all pipelines compile?" becomes a race against the
  frame count. Synchronous compilation makes the test **deterministic** and lets a small fixed
  frame budget guarantee every pipeline is resolved. To set it, the harness must override the
  `RenderPlugin` member of `DefaultPlugins` via `.set(RenderPlugin { synchronous_pipeline_compilation: true, ..default() })`.

### 2.3 GPU requirement

This test needs a real GPU adapter (`RenderPlugin` with the default `RenderCreation::Automatic`
backend). That is **acceptable** — the dev machine has an RTX 5080 (the smoke-runs in
`10-impl-b.md` already run there). It will **not** run on a GPU-less CI box: the test must be
written so it is obvious it is a GPU test. Strategy: gate it behind a `gpu` feature OR detect
adapter-creation failure and `eprintln! + return` (a skipped-not-failed test) — see §9 risk R1.
Recommended: a `#[cfg_attr(not(feature = "e2e-gpu"), ignore)]` on the test fn plus a
`e2e-gpu` feature in `Cargo.toml`, so `cargo test` on the dev box runs it explicitly
(`cargo test --features e2e-gpu`) and bare `cargo test` elsewhere skips it cleanly. The impl
agent's verification command becomes `cargo test --features e2e-gpu`.

### 2.4 What the harness must *not* pull in

- No `FreeCameraPlugin` input handling matters (no window → no input events → camera never
  moves → deterministic; see §5). The harness can still add `FreeCameraPlugin` for parity, or
  omit it — omitting is cleaner. **Omit `FreeCameraPlugin`** in the test app: the camera is
  static by design (§5) and `FreeCamera` is not read by any render system.
- No `hud.rs` — the HUD is a UI overlay drawn after the blit; it is irrelevant to the render
  gates and pulls in font assets. **Omit `setup_hud` / `update_hud`.** (`bevy_ui` etc. still
  load via `DefaultPlugins`, harmlessly.)
- DLSS: the test app does **not** insert `DlssProjectId` and does not add DLSS camera
  components. Phase B keeps DLSS dormant (`01-context.md` §2d); the headless test stays on the
  `force_disable_dlss`-equivalent path by simply not wiring it. Build the test with
  `--no-default-features` is **not** required — `DlssProjectId` is only consulted if inserted;
  not inserting it is enough.

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

Instead, after the fixed frame loop, the harness reaches into the `RenderApp` sub-world and
**iterates `PipelineCache::pipelines()`** (`pipeline_cache.rs:221` — public, yields
`&CachedPipeline`; `CachedPipeline { descriptor, state }` at `:39-42`). For each pipeline whose
`state` is `CachedPipelineState::Err(ShaderCacheError)` (`:46-55`), the harness collects
`(descriptor label / shader path, error)` and the test **`panic!`s with the full list**. The
`ShaderCacheError` `Display` carries the naga-oil / wgpu validation message — the same text the
live run logs via `error!` (`pipeline_cache.rs:699-705`).

Accessing the `RenderApp` world from the test:
```text
let render_app = app.sub_apps_mut().get_mut(RenderApp).unwrap();   // app.rs:1196 sub_apps_mut + 1251 region
let cache = render_app.world().resource::<PipelineCache>();
```
(`PipelineCache` is a render-world resource; `app.sub_apps()` / `sub_apps_mut()` are public —
`bevy_app-0.19.0-rc.1/src/app.rs:1191-1197`.)

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
`DeviceLost`.

Bevy routes wgpu errors through its `RenderDevice`; the cleanest test-side capture is to
install an **uncaptured-error scope** is not exposed by Bevy's `RenderDevice` wrapper, so the
harness instead relies on two cheaper signals that are sufficient in practice:

1. **Panic propagation.** A `DeviceLost` or a failed `queue.submit` surfaces as a panic inside
   `app.update()`; the test inherits it and fails. (Batch 1 bug #3's TDR manifested as exactly
   this — `DeviceLost`/swapchain `Timeout`.) The harness does **not** catch-unwind; a panic in
   `update()` *is* the test failure, with the wgpu message in the panic payload.
2. **The readback sanity check (§7).** A pipeline that "compiled" but is mis-fed produces a
   degenerate framebuffer (all-zero, all-NaN-encoded, or unchanged-from-clear). The §7
   assertions catch that as a content failure even when no error was raised.

Layer B is best-effort; Layer A is the load-bearing check. Together they cover the
`10-impl-b.md` bug catalogue.

### 3.3 Forcing every pipeline to be created

A pipeline is only queued when its render system runs and calls
`pipeline_cache.get_*_pipeline(id)` for the first time, AND the upstream resources exist (the
nodes early-return until `WorldGpu` / `FrameGpu` / `TaaGpu` / `AtmosphereGpu` / `GiGpu` exist —
`graph.rs:73-75`, `prepare.rs:302-310`). So "run a few frames" is not enough on its own — the
harness must run **enough frames that the prepare systems have built every resource and every
node has executed at least once**. Empirically that is small: the prepare systems build their
resources on the first valid frame, and `synchronous_pipeline_compilation` resolves the
pipelines the same frame they are queued. **Frame budget: 8 frames** (§4) is comfortably above
the resource-build latency (1 frame to extract the world, 1 to prepare GPU resources, 1 for the
first full graph execution) with margin for the camera-history ring to spin up. If a future
batch adds a pipeline that is only queued conditionally, the harness's per-batch assertion
(§6) for that batch must ensure the condition holds in the test scene.

---

## 4. Frame loop & determinism

### 4.1 The loop

```text
for _ in 0..E2E_FRAME_COUNT {        // E2E_FRAME_COUNT = 8 (a module const)
    app.update();
}
```

`app.update()` ticks the main world (Startup on frame 0, then Update) and the `RenderApp`
(Extract → Prepare → Render → the `Core3d` graph). No `app.run()`, no winit, no real time
pacing — the loop is as fast as the GPU allows.

### 4.2 Determinism strategy — exactly how

Every non-deterministic input the render path consumes is pinned:

| input | how it is made deterministic |
|---|---|
| **Camera pose** | The test spawns the camera at a **fixed `Transform`** (a const in the test, e.g. the `setup_camera` pose `Transform::from_xyz(11,7,17).looking_at((0,4,-3), Y)` — `camera/mod.rs:40`, or a test-specific pose chosen to frame the gates in §6). `FreeCameraPlugin` is **omitted** (§2.4), and with no window there are no input events, so the `Transform` never changes. `sync_position_split` (`camera/position_split.rs:102`) is a pure function of the `Transform` → `PositionSplit` is deterministic. |
| **Frame counter** | `CameraHistory.frame_count` is a monotonic integer counter incremented by `update_camera_history` (`render/taa.rs` — `06-design-a2.md` §9; the `05-review.md` §4 wall-clock-millis bug was fixed in A-2). After N `app.update()` calls it is exactly N. Not wall-clock-derived → identical every run. |
| **TAA jitter** | `halton_jitter(frame_count)` (`render/taa.rs`) is a pure function of the integer frame counter → deterministic. The jitter sequence is therefore identical run-to-run for a given frame index. **Do not disable TAA** — keep `AppArgs.taa = true` (the real path); determinism comes from the pinned frame counter, not from disabling jitter. (If a specific batch's gate proves jitter-sensitive at a region edge, that batch's assertion uses an interior region or a slightly relaxed tolerance — see §6 — rather than disabling TAA, which would stop exercising the TAA pipeline.) |
| **RNG / rand salt** | `GpuRenderParams.rand_counter` / `GpuGiParams.rand_counter*` are derived from `frame_count` (`prepare.rs:348`, `gi.rs` salt helpers) → deterministic given the pinned counter. The GI sampler's per-pixel noise is therefore the *same* noise every run. |
| **`Time` / `elapsed`** | No render-relevant system reads wall-clock `Time` (verified: `prepare.rs`, `taa.rs`, `position_split.rs`, `extract.rs` — none read `Time`/`elapsed` on the render path; the A-2 fix removed the last one). `ScheduleRunnerPlugin` advances `Time` per `update()` but nothing on the gate path consumes it. |
| **Viewport size** | The off-screen `Image` target is created at a **fixed size** — `E2E_WIDTH × E2E_HEIGHT`, a small fixed resolution (recommend **256×256** — large enough for stable regions in §6, small enough for a fast readback and fast GI dispatch). `extract_camera` reads `physical_viewport_size()` from the `Image` target → fixed. All the `pixel_count`-sized buffers (`first_hit_data`, `final_color`, the GI buffers) are therefore a fixed size every run. |
| **Test scene** | The existing `setup_test_grid` (`voxel/grid.rs:35`) builds the `GridPreset::Default` grid procedurally with **no RNG** (verified: `build_default_volume` / `build_palette` are deterministic constructors — `voxel/grid.rs:75,125`). Reused as-is → the world geometry + the emissive block are bit-identical every run. |
| **Pipeline compilation** | `synchronous_pipeline_compilation: true` (§2.2) — no background-task race. |

Net: given the same binary, the readback framebuffer is **bit-identical** across runs. That is
what makes the §5 golden-vs-statistic decision a real choice rather than forced.

---

## 5. Framebuffer readback approach

### 5.1 The chosen mechanism — render to an `Image`, read the `Image` back

The camera's `RenderTarget` is an off-screen `Image` (§2.1). To get its pixels to the CPU the
harness uses Bevy's **`GpuReadbackPlugin` + `Readback` component**
(`bevy_render-0.19.0-rc.1/src/gpu_readback.rs`):

- `Readback::texture(image_handle)` (`gpu_readback.rs:86-90`) registers the target `Image` for
  readback; the plugin copies the texture to a `MAP_READ` buffer and, when the copy completes,
  **triggers a `ReadbackComplete` entity-event** carrying `data: Vec<u8>` (`gpu_readback.rs:114-127`,
  `:242` `main_world.trigger(ReadbackComplete { data, entity })`).
- The harness installs an observer (or polls a resource the observer writes) for
  `ReadbackComplete`, stashes the latest `Vec<u8>`, and after the frame loop decodes it.

Why this over a manual buffer-map readback pass:
- It is the **first-party Bevy mechanism**, already wired for extract/prepare/cleanup and the
  async map → `ReadbackComplete` handshake. A hand-rolled `copy_texture_to_buffer` + `map_async`
  + `device.poll` pass would re-implement exactly `gpu_readback.rs` and add a custom render node.
- It reads the **final blit target** — the same texture `naadf_final_blit_node` writes — so the
  test asserts on the *actual composited output*, exactly what the user sees on a live run.
- The target `Image` must be created with `TextureUsages::RENDER_ATTACHMENT | TEXTURE_BINDING |
  COPY_SRC` and `RenderAssetUsages::all()` (the `COPY_SRC` is what the readback plugin's
  texture→buffer copy needs; cf. `screenshot.rs:376` which adds `COPY_SRC` to a screenshot
  target).

### 5.2 Frame-timing of the readback

`GpuReadbackPlugin` does the copy every frame the `Readback` component is present and delivers
`ReadbackComplete` one or more frames later (async map). The harness keeps the `Readback`
component present for the whole loop and simply uses the **last** `ReadbackComplete` payload
received by the end of the loop. To guarantee one arrives: run the 8-frame loop, then run a
small **drain loop** — a few extra `app.update()` calls (e.g. up to 4) until the observer has
seen at least one `ReadbackComplete` — then assert. (The drain loop is bounded; if no readback
arrives within the bound, that itself is a test failure: the render path never produced a
frame.)

### 5.3 Decoding the bytes

The target `Image` format should be a **plain, predictable format** — recommend
`TextureFormat::Rgba8UnormSrgb` (or `Rgba8Unorm`). The blit pipeline (`naadf_final.wgsl`) is
specialised per the view target's main-texture format (`prepare_blit_pipeline`, `graph.rs:195-199`),
so an `Image` target with an 8-bit RGBA format gives a blit-pipeline variant and a `Vec<u8>`
that is trivially 4 bytes/pixel. The harness decodes `ReadbackComplete.data` as
`&[[u8; 4]]` indexed by `y * width + x`. (`ReadbackComplete::to_shader_type` exists for
structured buffers — not needed here; the texture readback is raw RGBA8.)

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
committed file (`tests/e2e/baselines/<batch>.hash` or a `const` table in the test) and is
updated *only* by the batch that intentionally changes the image. This is cheap, catches
accidental drift, and is not brittle because it is only asserted-equal where the design says
"unchanged".

### 6.2 The per-batch gate functions

Each batch's visual gate is one function `assert_batch_N(fb: &Framebuffer)` where `Framebuffer`
is a thin wrapper over the decoded `&[[u8;4]]` + dimensions, with helpers:
`region_mean(rect) -> [f32;4]`, `pixel(x,y)`, `fraction_brighter_than(rect, thresh)`,
`is_near(color_a, color_b, tol)`. The test scene (`GridPreset::Default`) has a known layout —
a ground slab, axis-aligned boxes, a sphere, and **one emissive box** (`voxel/grid.rs` doc
comment + `build_default_volume`). The camera pose is fixed (§4.2), so each feature occupies a
**known screen rectangle**. The gates:

- **Batch 2 gate (4-plane first-hit + atmosphere) — the manual "emissive white / others black /
  sky" gate, mechanised:**
  - The emissive-block screen region: `region_mean` is **near-white / high-luminance** (the
    emissive material is the only lit thing pre-GI).
  - A non-emissive solid block region: **near-black** (no bounce light yet — Phase B pre-GI;
    the `base/` first-hit gives non-emissive diffuse surfaces no direct light until GI).
  - The sky region (a screen corner that misses all geometry): **sky-colored** — not black, not
    white; assert it is within a broad tolerance of the expected atmosphere tint and that its
    luminance is in a mid band. (The Batch-2 image *is* the 4-plane first-hit with the full
    multiple-scattering atmosphere — `10-impl-b.md` Batch 2 verification.)
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
- **Batch 6 gate (TAA):** the image is temporally stable — run the loop, capture the readback at
  frame K and frame K+1 (two `ReadbackComplete`s), and assert the **per-pixel delta between
  consecutive frames is small** (TAA has converged — no per-frame shimmer). Also assert the
  blit source is back to `taa_sample_accum` implicitly via the image still passing the Batch-5
  brightness gates. Re-bless the stability hash.

### 6.3 Tolerances

All gates use **generous tolerances** (region means, luminance bands, fractions-of-pixels —
not exact pixel equality). The point is to catch "the emissive block went black" or "GI never
turned on" or "the sky is now solid magenta because a bind group is mis-wired" — gross,
batch-level regressions — not sub-percent shading drift. Exact-pixel comparison is reserved for
the optional stability hash, and only where the design says the image must not change.

### 6.4 Extensibility — adding a batch is a small, obvious edit

The harness is structured so a new batch adds:
1. one `assert_batch_N(&Framebuffer)` function (a handful of region asserts),
2. one line in the dispatch table mapping the current batch number → its assert fn,
3. if the batch changes the image, one re-blessed hash baseline entry.

The headless-boot, frame-loop, readback, and pipeline-error-scan code is **batch-agnostic and
written once**. The "current batch" is a single `const CURRENT_BATCH: u32` the impl agent bumps
when a batch lands (or the test simply always runs the highest-implemented batch's gate plus
the pipeline-error scan). Recommended: the test runs **the pipeline-error scan + node-dispatch
check unconditionally** (every batch benefits) and **the highest batch's region gate**; older
batches' region gates are kept as called helpers so a regression in an earlier gate still trips.

---

## 7. Readback sanity floor (degenerate-frame guard)

Independent of the per-batch gates, every run asserts the readback is **not degenerate**:
not all-identical-pixels (a stuck clear color), not all-zero (nothing rendered), and contains
both some dark and some bright pixels (geometry + sky present). This catches the "pipeline
silently `return`ed so the frame is the clear color" failure mode (§3.1) even before the
per-batch gate runs, and gives a clearer message ("framebuffer is uniformly black — the render
graph produced no output" vs. a confusing region-mean assertion).

---

## 8. Render-graph node-dispatch check

The brief requires (item 6) that batches with no visible change still assert "all expected
render-graph nodes dispatch." Mechanism: the nodes already wrap their work in a
`time_span(encoder, SPAN)` (`graph.rs:88`, and every `graph_b.rs` node has a `*_SPAN` const).
`RenderDiagnosticsPlugin` surfaces each as a `render/<span>/elapsed_cpu` (and `_gpu`)
diagnostic (`hud.rs:14-29` documents the path scheme). The harness:

- adds `RenderDiagnosticsPlugin` (and `FrameTimeDiagnosticsPlugin`) to the test app,
- after the frame loop, reads the `DiagnosticsStore` and asserts that **every expected span for
  the current batch has a recorded measurement** — i.e. the node actually ran (a node that
  early-returns because its pipeline failed records *no* span).

The "expected spans for batch N" is a small `const &[&str]` table next to the per-batch assert
functions — extends exactly like §6.4. This is a second, cheaper signal that complements the
§3.1 pipeline-error scan: §3.1 says "the pipeline is broken," the dispatch check says "the node
that uses it never ran."

---

## 9. Where the test code lives + file structure

A Bevy app boot is heavyweight; it belongs in an **integration test** (`tests/`), not a
`#[cfg(test)]` unit module (the existing unit tests in `src/**` stay as they are — pure-CPU,
fast). The crate is currently binary-only with no `tests/` dir; this adds one.

```
tests/
  e2e_render.rs            the integration test entry — #[test] fns, the per-batch dispatch.
  e2e/
    mod.rs                 (declared from e2e_render.rs via `mod e2e;`)
    harness.rs             build_headless_app(), run_frames(), scan_pipeline_errors(),
                           read_framebuffer(), assert_nodes_dispatched() — all batch-agnostic.
    framebuffer.rs         Framebuffer wrapper + region_mean / pixel / luminance / is_near /
                           fraction_brighter_than / stability hash.
    gates.rs               assert_batch_2 .. assert_batch_N, the EXPECTED_SPANS tables, the
                           hash-baseline table, CURRENT_BATCH.
    baselines/             (optional) committed stability-hash files, if not kept as consts.
```

**Important — the binary crate must expose its app-wiring as a library surface the test can
call.** Today `main.rs` builds the app inside `fn main()`. The test cannot call `fn main()`.
Two options:

- **(Recommended) Add a `src/lib.rs`** that re-exports the modules (`aadf`, `camera`, `hud`,
  `render`, `voxel`, `world`, `AppArgs`, `GiSettings`, `GridPreset`) and a
  `pub fn build_app(config: AppConfig) -> App` that does the plugin/system wiring currently in
  `main.rs:102-161`, parameterised by an `AppConfig { headless: bool, render_target,
  add_hud: bool, add_free_camera: bool, .. }`. `main.rs` then becomes a thin
  `fn main() { build_app(AppConfig::windowed()).run(); }`. The integration test calls
  `build_app(AppConfig::headless_e2e())`. This is the clean, idiomatic Rust layout (a `lib.rs`
  + a thin `main.rs`) and keeps the test from duplicating ~60 lines of plugin wiring that would
  otherwise silently drift from `main.rs`.
- (Alternative, rejected) Duplicate the plugin/system wiring inside `harness.rs`. Rejected: it
  duplicates `main.rs` and *will* drift — the test would stop testing the real app.

This `lib.rs` extraction is a **prerequisite step** in the impl plan (§11 step 1). It is a pure
refactor (move code, no behavior change) and is small.

---

## 10. Methodology change — this REPLACES the live smoke-run as the agent's gate

**Binding process change, stated explicitly for every future Phase-B (and later) impl agent:**

- **Before:** impl-agent verification = `cargo build` + `cargo test` + a live `cargo run`
  smoke-run (open a window, watch for panics/validation errors/TDR for ~30s). The live run was
  the *only* thing that exercised WGSL/pipeline creation, and it cost N+ slow window-opening
  reruns to clear N shader bugs (`10-impl-b.md` Batch 1).
- **After:** impl-agent verification = `cargo build` + **`cargo test --features e2e-gpu`**
  (which includes this headless e2e render test). The e2e test exercises real WGSL composition,
  real pipeline creation, the real render graph, and the real framebuffer — in one shot, no
  window, fail-fast with the naga/wgpu error text. The impl agent iterates against `cargo test`
  alone.
- **The live windowed `cargo run` becomes the *user's* subjective review-gate check only** —
  the per-batch "user interactive re-test confirms GI is rendering / temporally stable / no
  artifacts" gate (`01-context.md` §2c, §2d done-bars). It is no longer part of the *agent's*
  loop. The agent ships when `cargo build` + `cargo test --features e2e-gpu` are green; the
  user then does the subjective visual pass.
- This also fixes the `subagent-gpu-app-verification-loop` memory hazard: the agent no longer
  rebuilds→reruns a windowed app chasing a visual outcome — it has a deterministic, one-shot
  test instead.

The impl agent that builds this harness must add a note to `10-impl-b.md` (and the orchestrator
should reflect it in the README checklist) recording the methodology change so subsequent batch
agents follow it.

---

## 11. Implementation plan (ordered — for the follow-up impl agent)

Each step ends compiling. Steps 1–7 build the batch-agnostic harness against the **current**
tree state (Batches 1–3 implemented); step 8 adds the gates for the batches that exist now.

1. **Extract `src/lib.rs`** (§9). Create `src/lib.rs` re-exporting the existing modules and a
   `pub fn build_app(cfg: AppConfig) -> App` carrying the wiring from `main.rs:102-161`,
   parameterised (`headless`, `render_target`, `add_hud`, `add_free_camera`,
   `synchronous_pipeline_compilation`). Rewrite `main.rs` to
   `fn main() { bevy_naadf::build_app(AppConfig::windowed()).run(); }`. `cargo build` + existing
   `cargo test` stay green (pure refactor).
2. **`Cargo.toml`** — add an `e2e-gpu` feature (empty — just a test gate) and, if needed, a
   `[[test]]` entry for `e2e_render` (Cargo auto-discovers `tests/*.rs`, so an explicit entry is
   only needed to set `harness = true`/name; default is fine — likely no `Cargo.toml` test entry
   needed beyond the feature).
3. **`tests/e2e/harness.rs`** — `build_headless_app()`: calls `bevy_naadf::build_app` with
   `AppConfig::headless_e2e()` (DefaultPlugins minus `WinitPlugin`, `ScheduleRunnerPlugin::run_once()`,
   `RenderPlugin { synchronous_pipeline_compilation: true }`, no HUD, no free camera), spawns the
   off-screen `Image` target (256×256, `Rgba8UnormSrgb`, `RENDER_ATTACHMENT|TEXTURE_BINDING|COPY_SRC`,
   `RenderAssetUsages::all()`), spawns the camera with `RenderTarget::Image` + a fixed `Transform`
   + `PositionSplit::from_world(..)` + `Msaa::Off`, adds `GpuReadbackPlugin`, attaches a
   `Readback::texture(handle)` + a `ReadbackComplete` observer that stashes the latest payload
   into a resource. Also `run_frames(&mut app, n)`, and `drain_until_readback(&mut app, max)`.
4. **`tests/e2e/harness.rs` (cont.)** — `scan_pipeline_errors(&App) -> Result<(), String>`:
   reach into `RenderApp` via `app.sub_apps()`, get `PipelineCache`, iterate `.pipelines()`,
   collect every `CachedPipelineState::Err` with its descriptor label + `ShaderCacheError`
   message. `assert_nodes_dispatched(&App, &[&str])`: read `DiagnosticsStore`, assert each
   expected `render/<span>/elapsed_cpu` has a measurement.
5. **`tests/e2e/framebuffer.rs`** — `Framebuffer { data: Vec<[u8;4]>, w, h }` + `from_readback`,
   `region_mean`, `pixel`, `luminance`, `is_near`, `fraction_brighter_than`, `stability_hash`
   (a stable hash, e.g. FxHash/`DefaultHasher` over the bytes), plus a `is_degenerate` check
   (§7).
6. **`tests/e2e/gates.rs`** — the `Framebuffer` known-rectangle constants for the
   `GridPreset::Default` scene at the fixed camera pose (emissive-block rect, solid-block rect,
   sky rect — derived once by the impl agent from a debug dump of the readback), the
   `EXPECTED_SPANS` table, the hash-baseline table, `CURRENT_BATCH`.
7. **`tests/e2e_render.rs`** — the `#[test]` (`#[cfg_attr(not(feature="e2e-gpu"), ignore)]`):
   `let mut app = build_headless_app(); run_frames(&mut app, 8); drain_until_readback(&mut app, 4);`
   then **unconditionally**: `scan_pipeline_errors` (panic on any Err with the messages),
   `is_degenerate` floor check, `assert_nodes_dispatched` for the current batch's spans; then
   the current batch's `assert_batch_N` region gate (+ stability-hash assert for B3/B4).
8. **Gates for the implemented batches** — write `assert_batch_2` (emissive-white/solid-black/
   sky), `assert_batch_3` + `assert_batch_4` (= Batch-2 gates + stability hash), wire the
   `EXPECTED_SPANS` for the Batches 1–3 node set (`naadf_atmosphere`, `naadf_first_hit`,
   `naadf_ray_queue`, `naadf_global_illum`, `naadf_final_blit`). Bless the initial stability
   hash from a first green run. Set `CURRENT_BATCH = 3`.
9. **Verify** — `cargo test --features e2e-gpu` is green on the dev box; `cargo test` (no
   feature) skips the e2e test cleanly. Add the §10 methodology note to `10-impl-b.md` and flag
   it to the orchestrator for the README checklist.
10. **Per-batch upkeep (not this agent — the rule for future batch agents)** — each subsequent
    batch (B4 onward, B5, B6) adds its `assert_batch_N`, its `EXPECTED_SPANS` row, bumps
    `CURRENT_BATCH`, and re-blesses the stability hash *iff* the batch intentionally changes the
    image (B5, B6 do; B4 does not). This is the §6.4 small-obvious-edit.

---

## 12. Open questions & risks

- **R1 — GPU-less environments.** The test needs a real adapter; it cannot run in GPU-less CI.
  Mitigation in §2.3 (the `e2e-gpu` feature gate + `#[ignore]` otherwise). **Open question for
  the orchestrator/user:** is the `feature`-gate acceptable, or should the test instead
  *detect* adapter-creation failure at runtime and self-skip with an `eprintln!`? The
  feature-gate is cleaner and recommended; confirm before the impl agent hard-codes it.
- **R2 — `ReadbackComplete` latency.** The async texture→buffer map may take more than one frame
  to deliver. §5.2's bounded drain loop handles it, but if the GPU readback never completes
  within the bound the test fails with "no framebuffer produced" — which is *correct* (the
  render path is broken) but the impl agent should pick a generous drain bound (4 frames) and
  not a tight one. Low risk.
- **R3 — Pipeline-creation is lazy.** §3.3: a pipeline is only queued once its node runs with
  all upstream resources present. The 8-frame budget is an empirical estimate (resource-build
  latency is ~3 frames). If a future batch adds a pipeline gated on a runtime condition (e.g.
  the denoiser node gated on `is_denoise`), the test scene must hold that condition true — the
  `GiSettings::default()` has all bools `true` (`main.rs:81-86`), so the default config already
  exercises every conditional Phase-B node. Worth a re-check when B5's denoiser lands.
- **R4 — Layer-B device-error capture is best-effort.** Bevy's `RenderDevice` wrapper does not
  expose a wgpu uncaptured-error scope to test code (verified — no such API on the 0.19-rc.1
  wrapper). The harness relies on panic-propagation + the readback content gates (§3.2). This
  caught Batch 1 bug #3 (it TDR'd → `DeviceLost` panic) in practice, but a *silent* validation
  error that neither panics nor visibly corrupts the frame would slip past Layer B. Layer A (the
  pipeline-error scan) is unaffected and remains the load-bearing check. **Open question:** is
  best-effort Layer B acceptable, or should the impl agent additionally install a custom render
  node that calls `device.poll(Maintain::Wait)` and checks — this is more invasive and probably
  not worth it; flagging for the orchestrator's call.
- **R5 — Known-rectangle constants are camera-pose-coupled.** The §6.2 region rectangles are
  derived from the fixed camera pose + the `GridPreset::Default` scene. If the test camera pose
  or the test grid changes, the rectangles must be re-derived. Mitigation: keep the test camera
  pose a single named const in `gates.rs`, document that the rectangles are derived from it,
  and have the impl agent derive them from an actual debug dump (write the first readback to a
  PNG once, eyeball the rects) rather than guessing. Not a blocker — just a maintenance note.
- **R6 — `synchronous_pipeline_compilation` on the test app only.** The windowed `main.rs` app
  should keep async pipeline compilation (no startup hitch). The `AppConfig` parameterises this
  (§11 step 1) so only the headless e2e config flips it on. Confirmed safe — it is a per-`RenderPlugin`
  flag, not global.
