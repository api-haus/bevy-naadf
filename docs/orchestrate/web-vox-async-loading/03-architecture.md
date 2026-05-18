# 03-architecture — web-vox-async-loading
2026-05-18

## Summary

The design unifies the web and native `.vox` install pipelines around two
shared seams: (i) an off-main-thread parse that produces a `Send`able
`ImportedVox` and is delivered back to the main thread through a poll-in-Update
channel, and (ii) a cross-frame state-machine readback that replaces the
sync `Device::poll(wait_indefinitely)` panic site at
`crates/bevy_naadf/src/render/construction/mod.rs:983-984`. Per Decision 2
the web `.vox` route is identical to native through the same
`install_vox_bytes_in_fixed_world` → W5 GPU producer chain → CPU mirror
readback; per Decision 3 the web parse uses `wasm-bindgen-rayon` with the
build configuration mirrored from `/mnt/archive4/DEV/bevy_pixel_world`.

- **Q1** — Web parse: `wasm-bindgen-rayon` worker pool + `rayon::spawn` +
  `crossbeam-channel` oneshot back to the Bevy main loop.
- **Q2** — Native parse: `bevy::tasks::AsyncComputeTaskPool::get().spawn(...)`
  + poll-in-`Update`. Same machinery shared with web (web's rayon shim
  feeds the same `crossbeam-channel` resource), one polling system.
- **Q3** — Async GPU readback: **cross-frame state machine** in the
  render-world `ExtractSchedule`. Issue `copy_buffer_to_buffer` + `map_async`
  in frame N, poll `BufferState::Mapped` (via `Device::poll(PollType::poll())`
  per frame) in subsequent frames, read mapped range + write CPU mirror
  in frame N+K. Works identically on native and WebGPU.
- **Q4** — Untouched (Decision 1). Implementer adds one debug-assertion
  documenting the three flagless W2 placeholders are dead on the `.vox` path.
- **Q5** — New native gate `--vox-web-parity` with three sub-modes
  (`--vox-web-parity-skybox`, `--vox-web-parity-loaded`, top-level). New
  `GridPreset::Empty` variant + extension of `vox_gpu_oracle.rs`'s
  SSIM template inverted to assert **dissimilarity** (SSIM < threshold).
  Error counter via a custom `tracing` Layer installed in `add_e2e_systems`.
  Wall-clock budgets on every wait loop.
- **Q6** — Playwright SSIM via a `--ssim-compare` flag on the existing
  `e2e_render` binary (no new crate, no new bin).
- **Q7** — The `#[cfg(target_arch = "wasm32")]` block at
  `crates/bevy_naadf/src/render/construction/mod.rs:944-957` is deleted.
  Q3's cross-frame readback replaces it with no wasm32-specific branch.

The implementer applies these in the order under
[Implementer ordering](#implementer-ordering-recommended).

## Q1 — Async parse on web (wasm-bindgen-rayon)

**Route.** Add `wasm-bindgen-rayon = "1.3"` as a wasm32-target dep, pin a
nightly toolchain with `rust-src`, install the `+atomics,+bulk-memory`
rustflags + shared-memory linker args, and re-export `init_thread_pool`
from `voxel::web_vox`. JS bootstrap calls `await initThreadPool(...)`
between `await init(...)` and dispatching `TrunkApplicationStarted`. The
stage-2 install body of `apply_pending_vox`
(`crates/bevy_naadf/src/voxel/web_vox.rs:307-338`) is replaced: instead of
calling `install_vox_bytes_in_fixed_world` synchronously, it spawns a
rayon task that runs `vox_import::parse_vox_bytes(&bytes)` and pushes the
`ImportedVox` through a `crossbeam_channel` bounded channel. A new Bevy
`Update` system (shared with Q2 on native — same channel type, same
poller) `try_recv()`s the parsed `ImportedVox` each frame and runs the
small remaining Bevy-resource-install half of
`install_vox_bytes_in_fixed_world` (no parse, just `commands.insert_resource`
calls).

**Build-config delta — exact lines to add.**

| File | Action | Content (copy from `/mnt/archive4/DEV/bevy_pixel_world`) |
|------|--------|----------------------------------------------------------|
| `.cargo/config.toml` (NEW at worktree root) | create | `[target.wasm32-unknown-unknown]` block with `rustflags = ["--cfg", "getrandom_backend=\"wasm_js\"", "-C", "target-feature=+simd128,+atomics,+bulk-memory,+mutable-globals", "-C", "link-arg=--shared-memory", "-C", "link-arg=--max-memory=1073741824", "-C", "link-arg=--import-memory", "-C", "link-arg=--export=__wasm_init_tls", "-C", "link-arg=--export=__tls_size", "-C", "link-arg=--export=__tls_align", "-C", "link-arg=--export=__tls_base"]` + `[unstable] build-std = ["std", "panic_abort"]`. Source: `/mnt/archive4/DEV/bevy_pixel_world/.cargo/config.toml:8-25`. |
| `rust-toolchain.toml` (EDIT) | replace | Channel `nightly-2025-11-15`, components `["rustfmt", "clippy", "rust-src"]`, targets `["wasm32-unknown-unknown", "wasm32-unknown-emscripten"]`. Bevy 0.19's MSRV is satisfied by nightly-2025-11-15 (the same toolchain `/mnt/archive4/DEV/bevy_pixel_world/rust-toolchain.toml` ships with Bevy 0.17). |
| `crates/bevy_naadf/Cargo.toml` (EDIT — `[target.'cfg(target_arch = "wasm32")'.dependencies]` at :119-147) | add deps | `wasm-bindgen-rayon = "1.3"`, `rayon = "1.11"`, `crossbeam-channel = "0.5"`. `crossbeam-channel` is already transitive (used by `wasm-bindgen-rayon`); make it direct so the install-side polling system has a clean import path. |
| `crates/bevy_naadf/index.html` (EDIT — replace the bottom `<script>` block, currently at :140-189) | edit | Move the wasm-init logic into a separate script that calls `await init(...)` then `await wasm.initThreadPool(navigator.hardwareConcurrency)` before dispatching `TrunkApplicationStarted`. **Dev (`trunk serve`) note:** Trunk's auto-injected loader does not call `initThreadPool` for us, so we replace `<link data-trunk rel="rust" data-bin="bevy-naadf" ...>` with a manual `<link data-trunk rel="rust" data-bin="bevy-naadf" data-loader-shim="..."/>` style is **not** available in Trunk 0.21. Instead: set `data-no-import="true"` on the existing link (this prevents Trunk's auto-import), and add a separate `<script type="module">` block that imports the wasm-bindgen JS, awaits `init`, awaits `initThreadPool`, then dispatches `TrunkApplicationStarted`. The production `init.js.template` (`crates/bevy_naadf/init.js.template:17-22`) is edited symmetrically — after the streaming `init({ module_or_path: b })` add `await window.wasmBindings.initThreadPool(navigator.hardwareConcurrency)` before `dispatchEvent(...TrunkApplicationStarted...)`. |
| `crates/bevy_naadf/init.js.template` (EDIT — at :21-22) | edit | After `const wasm = await init({...})`, add: `await bindings.initThreadPool(navigator.hardwareConcurrency)`. Then `window.hideLoading()` and dispatch as before. |
| `crates/bevy_naadf/_headers` (already correct at :7-9) | none | COOP/COEP already enabled — verified. |
| `e2e/serve.mjs` (already correct at :46-48) | none | COOP/COEP already set. |

**Re-export from Rust.** `crates/bevy_naadf/src/voxel/web_vox.rs` adds at the top:
```rust
#[cfg(target_arch = "wasm32")]
pub use wasm_bindgen_rayon::init_thread_pool;
```
This makes `initThreadPool` visible on the JS `wasmBindings` import. Without
the re-export the JS-side `wasm.initThreadPool(...)` call is undefined.

**Dispatching parse onto the rayon pool.** New module function in
`web_vox.rs`:
```rust
fn spawn_parse_task(bytes: Vec<u8>, source_label: String) {
    let sender = PARSE_RESULT_TX.with(|c| c.borrow().clone());
    rayon::spawn(move || {
        let parsed = crate::voxel::vox_import::parse_vox_bytes(&bytes)
            .map(|imp| (imp, source_label.clone()));
        // The receive end is held by the Update system; sender survives
        // for the lifetime of the wasm module.
        let _ = sender.send(parsed);
    });
}
```
`rayon::spawn` puts the work on a real worker thread (one of the
`navigator.hardwareConcurrency` Web Workers `wasm-bindgen-rayon` spawned).
`parse_vox_bytes` (`crates/bevy_naadf/src/voxel/vox_import.rs:154-157`) is
pure CPU + `Send`, drop-in compatible.

**Plumbing the result back to the main thread.** Replace the single-slot
`QUEUED_FOR_INSTALL` (`web_vox.rs:48-49`) with a `crossbeam_channel::bounded(1)`
pair:
```rust
thread_local! {
    static PARSE_RESULT_TX: RefCell<crossbeam_channel::Sender<...>> = ...;
    static PARSE_RESULT_RX: RefCell<crossbeam_channel::Receiver<...>> = ...;
}
```
`crossbeam_channel`'s `Sender`/`Receiver` are `Send + Sync`, work on wasm
with `+atomics`, and the wasm-bindgen-rayon crate already depends on them
(`/tmp/wasm-bindgen-rayon-1.3.0/Cargo.toml:52-53`). The Bevy `Update`
system (`apply_pending_vox`) reads `PARSE_RESULT_RX.try_recv()` each frame;
on `Ok((imp, label))` it does the **install half only** (the Bevy-resource
inserts, ~30 lines of the existing 125-line `install_vox_bytes_in_fixed_world`
body; the `dot_vox::load_bytes` + `parse_dot_vox_data` lines move into the
rayon task). Refactor the existing
`install_vox_bytes_in_fixed_world` (`crates/bevy_naadf/src/voxel/grid.rs:325-450`)
into two functions:
- `parse_to_imported_vox(bytes: &[u8]) -> Result<(ImportedVox, ...), VoxImportError>` — owns lines 331-352 of grid.rs (the parse + error branches).
- `install_imported_vox(commands, imp: ImportedVox, source_label: &str)` — owns lines 354-450 (the resource inserts).

The existing public signature stays as a synchronous convenience wrapper
(used by `vox_gpu_oracle.rs` and `vox_e2e.rs` paths; they call it from
`Startup` and tolerate the block). The async paths call `install_imported_vox`
after their respective task pumps have produced the `ImportedVox`.

**Two-stage overlay tied to rayon task lifecycle.**
- The existing stage-1 (`web_vox.rs:329-338`) stays — when bytes land it
  paints `"Parsing model…"` overlay text and `QUEUED_FOR_INSTALL` is replaced
  with a call to `spawn_parse_task(bytes, source_label)` (the rayon worker
  starts immediately).
- Stage-2 logic in the same `Update` system polls
  `PARSE_RESULT_RX.try_recv()`. On `Ok(Ok((imp, label)))` runs
  `install_imported_vox` + `hide_loading_overlay()`. On `Ok(Err(e))` logs the
  parse error + sets a "Parse failed" message + delayed hide (same shape as
  the current fetch-failure path at `web_vox.rs:266-289`).

The DOM overlay code (`web_vox.rs:88-119`) is untouched. `init.js.template`'s
streaming-fetch progress bar (`crates/bevy_naadf/init.js.template:17-21`) is
untouched. Indeterminate `#progress-fill.indeterminate` (`index.html:53-56`)
remains the visual signal for "parsing — no exact progress available".

**Wall-clock fallback.** rayon's `spawn` doesn't expose progress; the
overlay can't update during the multi-second parse. The overlay stays in
`.indeterminate` state for the parse duration (same UX as the current
sync path but the main thread is responsive).

**Cost.**
- **+nightly toolchain pin.** All workspace builds (`just build`,
  `cargo test`, `cargo run --bin e2e_render`) use nightly. CI's
  Cloudflare deploy uses the same pinned nightly.
- **+~10 MB wasm.** `build-std = ["std", "panic_abort"]` rebuilds the
  standard library with atomics; the rebuilt std is roughly twice the size
  of the prebuilt `wasm32-unknown-unknown` std. `wasm-bindgen-rayon` itself
  is ~50 KB.
- **+~10s build time on cold cache** (rebuilds std once, cached afterwards).
- **+1 JS module import.** The patched `init.js.template` calls
  `initThreadPool` once after `init`, ~50-200ms one-time cost.
- **0 runtime overhead** for non-parse paths. The thread pool sits idle
  until a parse fires.

## Q2 — Async parse on native

**Route.** `bevy::tasks::AsyncComputeTaskPool::get().spawn(...)` + a `Task<...>`
resource polled in `Update`. NOT a Bevy `AssetLoader<ImportedVox>` —
explanation in [Decisions](#decisions--rejected-alternatives).

**Applies to BOTH entry points** (`Startup` boot + `native_vox_drop_listener`)
via the same shared resource:

```rust
#[derive(Resource, Default)]
pub struct PendingVoxParse {
    pub task: Option<bevy::tasks::Task<Result<(ImportedVox, String), String>>>,
}
```

- **Startup path** (`crates/bevy_naadf/src/voxel/grid.rs:104-123`,
  `setup_test_grid`): when `GridPreset::Vox { path }` is matched and
  `args.vox_gpu_oracle_cpu_phase` is **false** (production path), the
  `install_vox_in_fixed_world(&mut commands, path)` call is replaced with
  a call to a new function `spawn_native_vox_parse(commands, path)`:
  ```rust
  let pool = bevy::tasks::AsyncComputeTaskPool::get();
  let path_for_label = path.display().to_string();
  let path_for_read = path.clone();
  let task = pool.spawn(async move {
      let bytes = std::fs::read(&path_for_read)
          .map_err(|e| format!("read failed: {e}"))?;
      let (imp, label) = parse_to_imported_vox(&bytes, &path_for_label)
          .map_err(|e| format!("parse failed: {e}"))?;
      Ok((imp, label))
  });
  commands.insert_resource(PendingVoxParse { task: Some(task) });
  ```
  In the meantime `setup_test_grid` calls
  `install_default_embedded_in_fixed_world` (the embedded default — so
  the world is renderable while the .vox parse runs in the background).
  This is a **behavioural change** vs the current sync `install_vox_in_fixed_world`,
  which atomically swaps the world at Startup with no embedded-default
  flash. Justification: it brings native parity with the web behaviour
  (which already flashes the embedded default scene; see
  `web_vox.rs:271-276` comment). For the **e2e harness** the new
  `--vox-web-parity-loaded` sub-mode waits for the parse to complete
  before sampling its assertion, so the embedded-default flash is
  invisible to the gate.

- **Drag-drop path** (`crates/bevy_naadf/src/voxel/grid.rs:471-529`,
  `native_vox_drop_listener`): when `DroppedFile` fires for a `.vox`, run
  the same `spawn_native_vox_parse(commands, path_buf.clone())` instead
  of the current sync `std::fs::read` + `install_vox_bytes_in_fixed_world`.

- **Polling system** (NEW — same Update system as web's
  `apply_pending_vox` — Decisions §1):
  ```rust
  fn poll_pending_vox_parse(
      mut commands: Commands,
      mut pending: ResMut<PendingVoxParse>,
  ) {
      use bevy::tasks::block_on;
      use bevy::tasks::futures_lite::future;
      let Some(task) = pending.task.as_mut() else { return; };
      if let Some(result) = block_on(future::poll_once(task)) {
          pending.task = None;
          match result {
              Ok((imp, label)) => install_imported_vox(&mut commands, imp, &label),
              Err(e) => error!(".vox async parse failed: {e}"),
          }
      }
  }
  ```
  `poll_once` is the Bevy-idiomatic non-blocking poll. Registered in
  `crate::build_app` (`crates/bevy_naadf/src/lib.rs:717-724`): on native
  via the existing `cfg(not(target_arch = "wasm32"))` block and shares
  the same resource type with web (cfg gates around `Task` vs the
  crossbeam channel resource — see [Decisions](#decisions--rejected-alternatives)
  §1).

**Wall-clock budget on Startup.** None needed at runtime — the parse takes
as long as it takes; the embedded default scene is renderable in the
meantime. The e2e gate (Q5) sets a 60s timeout on the wait-for-install
loop.

**Cost.**
- 0 new deps. `bevy::tasks::AsyncComputeTaskPool` is already used
  transitively (`world/data.rs:811-813`).
- **+1 Bevy resource** (`PendingVoxParse`) and **+1 Update system**.
- **Behavioural delta vs current native sync path**: brief embedded-default
  flash before `.vox` installs (web already has this; native didn't).
  Faithful-port rule (`bevy-naadf-faithful-port-rule.md`): C# port has no
  web target so the rule is permissive on web. For native this is a
  divergence, but it is **off the visible-rendering path** in a way the
  user already accepts on web — and it is the price of unblocking the
  startup thread. Documented in `04-refactoring.md`.

## Q3 — Async GPU readback (works on BOTH targets)

**Route.** Cross-frame state machine. The function
`populate_cpu_mirror_from_gpu_producer`
(`crates/bevy_naadf/src/render/construction/mod.rs:897-1060`) is
refactored to issue work over multiple frames instead of issuing
`map_async` + `device.poll(wait_indefinitely)` + `get_mapped_range` in a
single sync frame.

**Refactor shape.** `ConstructionGpu` (`crates/bevy_naadf/src/render/construction/mod.rs`
around :186-210) gains:
```rust
pub struct CpuMirrorReadback {
    pub stage: ReadbackStage,
    pub staging: [Option<Buffer>; 4],  // [cursor, chunks, blocks, voxels]
    pub copied_sizes: [u64; 4],
    pub stage_started_frame: u32,
}

pub enum ReadbackStage {
    NotStarted,
    SubmittedCursor,    // map_async on cursor buffer issued
    MappedCursor,       // cursor mapped; now know voxels/blocks sizes
    SubmittedFullSet,   // map_async on chunks/blocks/voxels issued
    MappedFullSet,      // all four staging buffers mapped; commit + done
}
```
`gpu.cpu_mirror_readback` replaces the boolean `gpu.cpu_mirror_populated`
(or coexists with it as a sub-state). The single-shot function becomes a
per-frame state-machine tick:

- **Stage 0 — NotStarted.** Gated on `gpu.gpu_producer_has_run = true`
  and `model_data.is_some()` (the same gate as today at lines 913-924).
  Allocates the cursor staging buffer + records its copy_buffer_to_buffer
  + submits the encoder + calls `slice.map_async(MapMode::Read, ...)`.
  Calls `render_device.poll(PollType::poll())` (non-blocking — drains the
  callback queue without waiting). Advances to `SubmittedCursor`.

- **Stage 1 — SubmittedCursor.** Each subsequent render frame calls
  `render_device.poll(PollType::poll())` and then probes the buffer.
  Per wgpu docs the mapped-callback fires from within `poll`; the
  buffer is queryable via `buffer.map_state() == BufferMapState::Mapped`
  in subsequent frames (Bevy 0.19 exposes wgpu 25.x's
  `Buffer::map_state()` API). Since `map_state()` is not directly exposed
  on the wgpu re-export, we instead use a **flag set inside the
  map_async callback** (via `Arc<AtomicBool>`). When the flag flips true,
  read the cursor, allocate the three remaining staging buffers
  (chunks/blocks/voxels — sized from the cursor), record + submit a single
  encoder containing all three `copy_buffer_to_buffer` calls, call
  `map_async` on each, then `poll(PollType::poll())`. Advance to
  `SubmittedFullSet`.

- **Stage 2 — SubmittedFullSet.** Same flag-probe pattern; once all three
  flags fire (use one `Arc<AtomicU32>` counting completed maps, or three
  separate `Arc<AtomicBool>`s), read all three `slice.get_mapped_range()`s,
  commit the CPU mirror to `WorldData` (lines 1047-1057 today), drop
  + unmap staging buffers, advance to `MappedFullSet` (terminal).

**Why a separate flag instead of `slice.map_state()`.** wgpu 25 exposes
`Buffer::map_state()` (`MapState::Unmapped`/`Waiting`/`Mapped`) but the
Bevy 0.19 `render_resource::Buffer` re-export wraps it in a way that
historically doesn't expose `map_state()` (the inner wgpu buffer is
private). The
`AtomicBool`-set-from-callback pattern is wgpu-cookbook-canonical and
sidesteps the re-export issue. See [Assumptions](#assumptions-made) §4.

**No wasm32 escape hatch.** This same state machine drives both targets.
On native, `render_device.poll(PollType::poll())` is a real non-blocking
poll — fastest path; the cursor may complete in 1 frame (the first poll
after `submit` is enough to drive the queue). On WebGPU, `poll(Poll)` is
a no-op but the JS `mapAsync` promise resolves on the JS event-loop tick
that follows the `submit`; by the next render frame the callback has
fired and the `AtomicBool` is set. Expected steady-state latency:
~2 frames on native, ~2-4 frames on WebGPU. The CPU mirror's only consumer
is the **editor** (hash-keyed `set_voxel*`); the renderer reads `WorldGpu`
storage buffers directly (Q3 panic-site comment, mod.rs:933-936) and is
unaffected. The editor was already silently broken on web pre-this-design
(per the interim hack at :944-957); after this design the editor sees the
CPU mirror within ~4 frames of the GPU producer chain completing — well
within the bounds where the user could have moved the mouse to apply a
brush stroke.

**Wall-clock budget on the readback state machine.** The state machine
has no internal wait loop — each render frame ticks it once. If wgpu
never delivers the mapped callback (a real failure mode, e.g.
`DeviceLost`) the readback stalls indefinitely AT the stage that's
waiting. **Diagnostic bail per
`feedback-e2e-gates-must-fail-fast.md`:** after `STALL_FRAMES = 600` (~10s
at 60fps) without advancing past the current stage, emit a single
`error!("vox readback stalled at stage {:?} after {} frames")` and force
advance to `MappedFullSet` (i.e. mark the mirror "populated" with empty
CPU buffers so subsequent frames don't keep retrying). This matches the
existing behaviour of the wasm32 escape hatch (it sets
`cpu_mirror_populated = true` without populating) but only triggers on
true failure.

**Refactor — function signature.**
- `populate_cpu_mirror_from_gpu_producer` keeps its current signature
  (same Bevy system params: `MainWorld`, `ConstructionGpu`, `WorldGpu`,
  `ModelDataRender`, `RenderDevice`, `RenderQueue`). Internally it
  dispatches on `gpu.cpu_mirror_readback.stage`.
- The internal `readback_u32` closure at :966-990 is removed. Replaced
  with a small helper that records a copy + issues `map_async` + sets up
  the `AtomicBool` callback.
- The system stays registered in the same place
  (`mod.rs:2893` in `ExtractSchedule`).

**Cost.**
- **~150 lines net change** in `mod.rs`. Per-frame work stays cheap (≤4
  staging-buffer allocations one-time + ≤4 cheap `poll` calls per frame
  in active stages, 0 in `NotStarted`/`MappedFullSet`).
- 0 new deps. `Arc<AtomicBool>` is `std`.
- **+4 staging buffer allocations one-time** per `.vox` load
  (≤200 MB for the 4096³ worst case but typically <50 MB) — wgpu drops
  them on the staging-buffer pool naturally once unmapped + dropped.

## Q4 — Other W2 placeholder buffers (Decision 1: confirm untouched)

**Re-state of audit verdict.** The three flagless W2 placeholders —
`segment_voxel_buffer_w2_placeholder` (`mod.rs:1934-1942`),
`hash_map_w2_placeholder` (`mod.rs:1943-1951`), and
`hash_coefficients_w2_placeholder` (`mod.rs:1952-1960`) — are
**never allocated on the `.vox` production path**. The gate at
`mod.rs:1184-1186` sets `want_gpu_producer = construction_config.gpu_construction_enabled && (dense_data_ready || model_data_present)`,
and on `.vox` runs `model_data_present = true`, so the `gpu_producer`
allocation block at :1187-1323 runs FIRST. That block creates
`naadf_hash_map_gpu_producer` (with COPY_SRC at :1203),
`naadf_hash_coefficients_gpu_producer` (no COPY_SRC at :1236 — never
read back), `naadf_block_voxel_count_gpu_producer` (with COPY_SRC at
:1263), and the W5 `naadf_segment_voxel_buffer_w5` (with COPY_SRC at
:1574). When the placeholder block at :1916-1960 fires later in the
same `prepare_construction` call, each `if gpu.<buffer>.is_none()`
guard returns `false` (the gpu_producer block already populated those
slots), so the three flagless placeholders are dead code on the `.vox`
path.

**Implementer adds one assertion.** In `populate_cpu_mirror_from_gpu_producer`,
between the `Some(gpu)` extraction at :913 and the gate check at :914,
add a debug-mode assertion:
```rust
#[cfg(debug_assertions)]
if model_data.is_some() {
    // Decision 1 (`docs/orchestrate/web-vox-async-loading/01-context.md`):
    // the three flagless W2 placeholders MUST NOT be the buffers being
    // read back on the .vox path. The gpu_producer alloc block at
    // mod.rs:1187-1323 should have populated `hash_map`, `block_voxel_count`,
    // and `segment_voxel_buffer` with the W5 production buffers first.
    // If a future regression to the gate logic at :1184-1186 routes a
    // `.vox` run through the placeholder block, this assertion catches it.
    if let Some(buf) = gpu.block_voxel_count.as_ref() {
        let label = buf.label();
        assert!(
            !label.contains("w2_placeholder"),
            "block_voxel_count is the W2 placeholder on a .vox run — \
             gate logic regression at mod.rs:1184-1186. Label was: {label}"
        );
    }
}
```
`bevy::render::render_resource::Buffer` exposes `label()` per wgpu 25.
This is a debug-only assert; release builds skip it entirely. Three
buffer labels checked (the three that the audit confirmed are dead).

**Cost.** Tiny — ~12 lines of debug-only code. Zero runtime cost in release.

## Q5 — New native e2e gate

**Gate name.** `--vox-web-parity` (the brief's working name; kept).
**Sub-modes.** `--vox-web-parity-skybox` (skybox-baseline phase) and
`--vox-web-parity-loaded` (vox-loaded phase) mirror
`vox_gpu_oracle.rs`'s two-phase pattern at
`crates/bevy_naadf/src/e2e/vox_gpu_oracle.rs:346-463`.

**Skybox-empty mechanism.** New `GridPreset::Empty` variant in
`crates/bevy_naadf/src/lib.rs:65-78`:
```rust
pub enum GridPreset {
    #[default] Default,
    Vox { path: PathBuf },
    /// Skybox-only — install an empty fixed-world `WorldData` and skip
    /// the W5 GPU producer chain. Used by `--vox-web-parity-skybox` to
    /// capture a pixels-pure-sky baseline against which the vox-loaded
    /// phase's framebuffer is SSIM-compared (dissimilarity assertion).
    Empty,
}
```
`setup_test_grid` (`crates/bevy_naadf/src/voxel/grid.rs:104-123`) adds
the `GridPreset::Empty` arm:
```rust
GridPreset::Empty => {
    install_empty_world(&mut commands);
}
```
A new `install_empty_world` next to `install_default_embedded_in_fixed_world`
(`grid.rs:136`) inserts an EMPTY `WorldData` (no `ModelData` resource at all,
`dense_voxel_types = Vec::new()`, so `want_gpu_producer = false` at
`mod.rs:1185-1186`; the GPU producer chain doesn't run; the renderer reads
empty `WorldGpu` buffers and the framebuffer is pure sky).

Rejected: `AppArgs.skybox_only_phase: bool` — see
[Decisions](#decisions--rejected-alternatives) §3.

**Module layout.** New file `crates/bevy_naadf/src/e2e/vox_web_parity.rs`
modeled on `vox_gpu_oracle.rs` (Candidate #5 in audit; lines 471-582 +
642-686 are the SSIM template). Exports:
- `run_vox_web_parity_compare() -> u8` — top-level orchestrator (spawns
  the two sub-phase subprocesses, loads PNGs, runs `compare_dissimilar_frames`).
- `run_vox_web_parity_skybox_phase() -> AppExit` — boots with
  `GridPreset::Empty` + `AppArgs.vox_web_parity_skybox_phase = true`,
  captures `target/e2e-screenshots/vox_web_parity_skybox.png`.
- `run_vox_web_parity_loaded_phase() -> AppExit` — boots with
  `GridPreset::Vox { path: oasis }` + `AppArgs.vox_web_parity_loaded_phase = true`,
  exercises the new async parse + readback (Q2 + Q3), captures
  `target/e2e-screenshots/vox_web_parity_loaded.png`.
- `pin_vox_web_parity_camera` — same shape as `pin_vox_gpu_oracle_camera`
  at `vox_gpu_oracle.rs:642-659`. Camera pose: reuse
  `ORACLE_CAMERA_POS`/`ORACLE_CAMERA_LOOK` (`vox_gpu_oracle.rs:154-158`)
  — both phases use the same pinned pose so the only difference between
  skybox PNG and loaded PNG is "vox present vs absent".
- `compare_dissimilar_frames(skybox: &Framebuffer, loaded: &Framebuffer) -> Result<String, String>` —
  parallel sibling of `compare_oracle_frames` (`vox_gpu_oracle.rs:471-582`)
  with the assertion direction inverted (asserts `ssim < threshold` —
  see below). Sanity guards from the oracle reused on the loaded frame
  (geometry visible, scene not pure sky).
- `VoxWebParityState` resource — same shape as `VoxGpuOracleState` at
  `vox_gpu_oracle.rs:669-672`.

**SSIM threshold + direction.** Asserts the loaded frame is
**dissimilar enough** from the skybox baseline (i.e. the .vox actually
rendered something other than pure sky):
```rust
pub const VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX: f64 = 0.85;
```
Tuned conservatively: when the .vox is missing, skybox and loaded are
identical (SSIM = 1.0 — clearly fail). When the .vox renders normally,
the loaded scene is heavily voxel-filled and at the chosen above-world
camera pose has very different colour distribution from the gradient
sky; expected SSIM is **far below** 0.85 (likely < 0.5). 0.85 is the
conservative ceiling — if SSIM is **≥ 0.85** the geometry didn't
substantially affect the frame and the gate fails. **Direction:**
```rust
if ssim_score >= VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX {
    return Err(format!(
        "SSIM {ssim_score:.4} >= dissimilarity max {:.3} — loaded frame \
         is structurally too similar to the skybox baseline. The .vox \
         install path likely failed to populate the renderer.",
        VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX,
    ));
}
```
The threshold value will need a final empirical tune during impl — see
[Assumptions](#assumptions-made) §5.

**`tracing::error!` counter.** Implementer adds a custom
`tracing-subscriber::Layer` registered in
`crate::e2e::add_e2e_systems`. Implementation shape:

```rust
// crates/bevy_naadf/src/e2e/tracing_error_counter.rs (NEW)
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tracing::{Level, Subscriber};
use tracing_subscriber::Layer;

#[derive(Resource, Clone, Default)]
pub struct TracingErrorCounter(pub Arc<AtomicUsize>);

pub struct CountingLayer(pub Arc<AtomicUsize>);

impl<S: Subscriber> Layer<S> for CountingLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        if *event.metadata().level() == Level::ERROR {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }
}
```

Registered in `add_e2e_systems`
(`crates/bevy_naadf/src/e2e/mod.rs:200-264`):
```rust
let error_counter = TracingErrorCounter::default();
app.insert_resource(error_counter.clone());
// Layer registration: use tracing_subscriber's global registry.
// Bevy's LogPlugin already installs a Registry-based subscriber; we hook
// our counting Layer in by re-using the dispatcher (the alternative is
// to call `tracing_subscriber::registry().with(CountingLayer(...)).init()`
// before DefaultPlugins, but Bevy 0.19's LogPlugin already calls .init()
// — we have to attach via Bevy's `bevy_log::LogPlugin::custom_layer`
// hook instead).
```
**Wiring detail.** Bevy 0.19's `LogPlugin` exposes a `custom_layer:
fn(app: &mut App) -> Option<BoxedLayer>` field. The implementer sets
this hook on the e2e harness's `LogPlugin`. Bevy's LogPlugin then folds
the custom layer into its `tracing-subscriber` registry. The implementer
verifies the exact `LogPlugin` API field name (Bevy 0.19 docs) during
impl — see [Assumptions](#assumptions-made) §6.

The gate's `vox_web_parity::compare_dissimilar_frames` reads the counter
post-run and folds any non-zero count into the `Err(...)` verdict (same
mechanism as `PipelineScanResult` at `e2e/checks.rs:142`).

Rejected: widening `PipelineScanResult` (it counts pipeline errors, not
tracing errors — different signal); scoping to "pipeline errors only"
(brief explicitly asks for zero `tracing::error!` calls). See
[Decisions](#decisions--rejected-alternatives) §4.

**Wall-clock budgets + diagnostic bail (per
`feedback-e2e-gates-must-fail-fast.md`, inlined in 01-context.md).** Every
wait loop in the gate gets an `Instant::now()` budget:

| Loop | Location | Budget | Bail diagnostic |
|------|----------|--------|------------------|
| Wait for `.vox` async parse completion (loaded-phase only) | `e2e/driver.rs` new `VoxWebParityWaitParse` state (added next to `VoxGpuOracleWarmup` at :221) | 60s wall-clock | "vox parse never completed within 60s; PendingVoxParse.task still set, source label: {}, mirror state: {ConstructionGpu::cpu_mirror_readback.stage:?}" |
| Wait for CPU mirror readback completion (loaded-phase only) | same driver state | 30s wall-clock | "CPU mirror readback stalled at stage {:?} after 30s; staging buffers: {N} allocated" |
| Wait for screenshot capture (both phases) | existing `VoxWebParityDrain` state, modeled on `VoxGpuOracleDrain` at driver.rs:1471 | `E2E_DRAIN_FRAMES = 8` frames (already wall-clock-bounded indirectly via the 60fps tick rate ~133ms) | reuses the existing "capture never delivered within N drain frames" diagnostic |
| Subprocess spawn + wait | `run_vox_web_parity_compare` — same shape as `vox_gpu_oracle.rs:346-463` | inherits `std::process::Command::status()` which has no inherent timeout — implementer wraps each subprocess in `timeout 120s` env-or-shell, OR uses `Command::spawn() + Child::wait_timeout` (the `wait-timeout` crate). For this gate, use `subprocess.spawn().wait()` with no wrapper; the per-phase 60s budget inside the subprocess + the outer harness's `cargo test` timeout are sufficient. | inherits subprocess's diagnostic |

Per-phase warmup uses the existing `ORACLE_WARMUP_FRAMES = 120` constant
(reused from `vox_gpu_oracle.rs:178`).

**E2ePhase additions.** New phases on `crate::e2e::driver::E2ePhase`
(after :229):
```rust
VoxWebParityWarmup,
VoxWebParityWaitParse,    // loaded phase only — gated by PendingVoxParse + readback stage
VoxWebParityShoot,
VoxWebParityDrain,
```
Same shape as the `VoxGpuOracle*` phases. Skybox phase skips
`VoxWebParityWaitParse` (no parse to wait for) and goes
`Warmup → Shoot → Drain → Done` like the oracle phases. Loaded phase
goes `Warmup → WaitParse → Shoot → Drain → Done`.

**Pipeline-error scan.** Reuse the existing
`PipelineScanResult` (`crates/bevy_naadf/src/e2e/checks.rs:44`) — folds
pipeline errors into the `AppExit`. The new gate's verdict is
`(no_panics) AND (pipeline_scan_passes) AND (tracing_error_count == 0)
AND (ssim < threshold)`.

**`add_e2e_systems` registration delta.**
```rust
// crates/bevy_naadf/src/e2e/mod.rs:228 area
.init_resource::<vox_web_parity::VoxWebParityState>()
.insert_resource(TracingErrorCounter::default())
```
```rust
// crates/bevy_naadf/src/e2e/mod.rs:260-261 area
vox_web_parity::pin_vox_web_parity_camera
    .after(oasis_edit_visual::pin_oasis_camera),
```

**`bin/e2e_render.rs` registration delta.** Mirror the
`--vox-gpu-oracle` three-flag pattern (`bin/e2e_render.rs:114-116, 142-145,
279-290`):
```rust
let vox_web_parity_mode = args.iter().any(|a| a == "--vox-web-parity");
let vox_web_parity_skybox_mode = args.iter().any(|a| a == "--vox-web-parity-skybox");
let vox_web_parity_loaded_mode = args.iter().any(|a| a == "--vox-web-parity-loaded");
// At top of dispatch, before any `else if`:
if vox_web_parity_mode {
    return ExitCode::from(bevy_naadf::e2e::vox_web_parity::run_vox_web_parity_compare());
}
// In the `let app_exit = ...` chain:
} else if vox_web_parity_skybox_mode {
    bevy_naadf::e2e::vox_web_parity::run_vox_web_parity_skybox_phase()
} else if vox_web_parity_loaded_mode {
    bevy_naadf::e2e::vox_web_parity::run_vox_web_parity_loaded_phase()
}
```

**Cost.** Roughly:
- ~600 lines new code (`vox_web_parity.rs` + `tracing_error_counter.rs`
  + driver phase additions). ~80% is mechanically copy-and-adapt of
  `vox_gpu_oracle.rs` (a ~700-line template).
- 0 new external crate deps. `tracing` + `tracing-subscriber` are already
  transitive via Bevy.
- ~30s of e2e runtime (CPU + GPU phase subprocesses each warmup
  120 frames @ 60fps = 2s warmup; 60s budget for parse; 30s budget for
  readback; in practice the loaded phase takes ~10-15s with the Oasis
  fixture, the skybox phase ~3s).

## Q6 — Playwright SSIM gate

**Route.** **Add `--ssim-compare <a.png> <b.png> [--max <ssim>]` flag to
the existing `e2e_render` binary** (option b in the brief — single-bin
add a flag, no new bin).

**CLI shape + exit-code semantics.**
```
e2e_render --ssim-compare <a.png> <b.png> [--ssim-max <f64>] [--ssim-min <f64>]
```
- Exactly two positional PNG paths after `--ssim-compare`.
- Optional `--ssim-max <f64>` — fail if `ssim >= max` (dissimilarity
  gate, default 0.85; the Q5 threshold). Used by the Playwright spec for
  the "loaded vs skybox is dissimilar" check.
- Optional `--ssim-min <f64>` — fail if `ssim < min` (similarity gate,
  default 0.85; matches the `vox_gpu_oracle.rs:211` threshold). Provided
  for symmetry — Playwright doesn't use it but lets `e2e_render
  --ssim-compare` work both ways.
- If both `--ssim-max` and `--ssim-min` are supplied, the score must
  satisfy `min <= ssim < max` (band gate).
- Output: stdout prints `SSIM=<f64>` (one line), `WIDTH=<u32>`,
  `HEIGHT=<u32>` (for diagnostics).

**Exit codes:**
- `0` — gate passed.
- `1` — gate failed (SSIM out of asserted range).
- `2` — internal error (file not found, dimensions mismatch, image-compare
  internal error). Distinct from `1` so Playwright can tell "config error"
  apart from "real visual regression".

**Implementation site.** `bin/e2e_render.rs` adds a short-circuit
**before** booting the Bevy app (next to the existing `--vox-gpu-oracle`
short-circuit at `bin/e2e_render.rs:142-145`). The flag parser is ad-hoc
(matches the binary's existing flag-parsing style at :81-117). Body
calls into a new public function
`bevy_naadf::e2e::vox_web_parity::ssim_compare_command(args: &SsimArgs) -> ExitCode`
that:
1. Loads both PNGs via `image::open` + `to_rgba8` (same as
   `vox_gpu_oracle.rs:616-631`'s `load_png_as_framebuffer`; the
   implementer factors that helper out of `vox_gpu_oracle.rs` into
   `framebuffer.rs` so it's reusable — small refactor, no behaviour change).
2. Calls `image_compare::rgb_similarity_structure(Algorithm::MSSIMSimple, &a, &b)`.
3. Prints the diagnostic lines.
4. Exits per the rules above.

**Playwright integration.** The current spec
`e2e/tests/vox-loading.spec.ts:135-148` already captures a screenshot.
The extended spec:
1. **Skybox baseline run** (`test.beforeAll` or split into a dedicated
   `vox-loading-skybox.spec.ts`): boot the page with a new
   `?skybox=1` query string. `voxel::web_vox::startup_fetch_default_vox`
   reads the param (parallel to the existing `?vox=<url>` override at
   `web_vox.rs:142-153`) and **skips** the HTTP fetch + dnd install. The
   embedded default scene from `setup_test_grid` is replaced by the new
   `GridPreset::Empty` install path via a Rust-side check: when wasm boots,
   if `?skybox=1` is set, call into a new
   `voxel::grid::install_empty_world` instead of the normal
   `install_default_embedded_in_fixed_world`. Wait for 5s of stable
   rendering, screenshot canvas to `vox_web_parity_skybox.png`.
2. **Loaded run** (the existing spec body): boot with
   `?vox=/test-fixtures/oasis_hard_cover.vox`, wait for the
   install-complete INFO log + additional 10s for stable rendering,
   screenshot to `vox_web_parity_loaded.png`.
3. **SSIM compare step**: run `cargo run --bin e2e_render -- --ssim-compare
   <skybox_png> <loaded_png> --ssim-max 0.85` via Node `child_process.spawn`.
   Asserts exit code 0. On non-zero, attach both PNGs + stdout to the
   Playwright HTML report.

**Skybox-baseline-on-web mechanism.** Pick the `?skybox=1` query string
(NOT reusing the new `GridPreset::Empty` directly — `GridPreset` lives in
the lib's `AppArgs` resource which web doesn't exercise from CLI flags).
`voxel::web_vox::startup_fetch_default_vox` (`web_vox.rs:249-293`) reads
the param up-front:
```rust
fn resolve_skybox_only_param() -> bool {
    web_sys::window()
        .and_then(|w| w.location().search().ok())
        .map(|s| s.split('&').any(|p| p == "?skybox=1" || p == "skybox=1"))
        .unwrap_or(false)
}
```
If true, **skip** the HTTP fetch + suppress DND. Additionally — a Bevy
`Startup` system (NEW: `voxel::grid::apply_web_skybox_override`) on web
checks the same param via a thread_local set by the bootstrap shim and
overrides the default install to `install_empty_world`. Order matters:
this Startup system runs `.before(setup_test_grid)` — actually simpler:
expose a new resource `WebSkyboxOverride(bool)` inserted by
`startup_fetch_default_vox` before `setup_test_grid` runs, and
`setup_test_grid` checks it.

**Cost.**
- ~150 lines new code (CLI parser + ssim_compare_command +
  Playwright spec extension).
- 0 new Rust deps (image-compare + image already vendored at
  `bevy_naadf/Cargo.toml:62-72`).
- 0 new Node deps (`child_process.spawn` is stdlib).
- **Net new web-only feature**: `?skybox=1` URL param (small,
  bracketed by an `if` in `web_vox.rs`).

## Q7 — Interim hack removal

The `#[cfg(target_arch = "wasm32")]` block at
`crates/bevy_naadf/src/render/construction/mod.rs:944-957` is **deleted
verbatim** as part of Q3's refactor — the cross-frame state machine
replaces it and there is no wasm32-specific branch left in
`populate_cpu_mirror_from_gpu_producer`. The
"WebGPU divergence" preamble comment at :926-942 is also deleted (Q3's
state machine is documented in its own module-level comment block;
the panic-site rationale moves to that block).

After Q3 lands, the function body grep-tests clean against
`#[cfg(target_arch = "wasm32")]` (zero matches in this function);
`populate_cpu_mirror_from_gpu_producer` is portable.

## Decisions & rejected alternatives

1. **Q1 (web async parse): `wasm-bindgen-rayon` chosen, `bevy::tasks::AsyncComputeTaskPool` rejected as web mechanism.**
   - **Rejected option:** Use Bevy's own `AsyncComputeTaskPool::get().spawn(...)` on web. Hoped for "one code path for both targets". **Rejected because:** Bevy 0.19's `bevy_tasks` on wasm32 unconditionally uses `single_threaded_task_pool` (verified at `/home/midori/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/bevy_tasks-0.19.0-rc.1/src/single_threaded_task_pool.rs:30-33`) which calls `web_task::spawn_local` — **runs on the main thread**, no WebWorker, no parallelism. Does NOT unblock the main thread during the multi-second `parse_vox_bytes` call. This is the audit's borderline call #2 finding, now confirmed.
   - **Rejected option:** Dedicated JS WebWorker hand-rolled (a `worker.js` that hosts its own wasm instance + posts messages back). Heavier infra to wire up; doubles the wasm bundle size (each worker is a separate wasm instance unless shared-memory is enabled — at which point we're back at `wasm-bindgen-rayon`'s shape but DIY).
   - **Deciding factor:** `wasm-bindgen-rayon` is the canonical Rust solution for "real off-main-thread parallel work on web", proven in `/mnt/archive4/DEV/bevy_pixel_world` (config aspirationally present even though they use rayon for native paths only — see [Assumptions](#assumptions-made) §1), and the `+atomics`/COOP/COEP infrastructure is already 100% in place in bevy-naadf. The cost (nightly toolchain + build-std) is one-time + cached.

2. **Q2 (native async parse): `AsyncComputeTaskPool::spawn(...)` chosen, `AssetLoader<ImportedVox>` rejected.**
   - **Rejected option:** A Bevy `AssetLoader<ImportedVox>` (model after `crates/bevy-instamat/src/baked_material.rs:215-223` or `crates/bevy_naadf/src/texture_array/loader.rs`). **Rejected because:** (i) AssetLoader is path-driven — clean fit for the Startup boot path (`GridPreset::Vox { path }`) but **awkward** for the drag-drop entry point (`native_vox_drop_listener` already has the path; it'd have to call `AssetServer::load(<path>)` and then poll `AssetServer::get_state` per frame — yet another resource + system). (ii) Web's `web_vox::apply_pending_vox` already has the **bytes**, not a path — it can't drive through `AssetLoader::load_path` cleanly; would need an in-memory asset trick. (iii) AssetLoader returns an `Asset` (must be `Reflect + TypePath + Asset + Sync` — `ImportedVox` is none of these today); wrapping introduces a new asset type to register. (iv) AsyncComputeTaskPool gives a single shared `Task<...>` resource that drops in cleanly for both entry points (drag-drop bytes-in-hand, Startup path-in-hand) and naturally extends to web (where the `Task<...>` API is uniform — see [Assumptions](#assumptions-made) §1 about the cfg-gated polling shape).
   - **Deciding factor:** Same code shape between Startup boot, native dnd, and the web parse pump. The pump is one `Update` system gated on the same `PendingVoxParse` resource type; the cfg-shaped difference is internal (whether the inner `Future` is rayon-backed via crossbeam or AsyncComputeTaskPool-backed). Saves the implementer one whole subsystem.

3. **Q3 (async readback): Cross-frame state machine chosen, await-style via oneshot rejected.**
   - **Rejected option:** Make `populate_cpu_mirror_from_gpu_producer` async, drive it from `AsyncComputeTaskPool::get().spawn(...)`, await the mapping callback via a `futures-channel::oneshot`. **Rejected because:** (i) The render-world's `ExtractSchedule` is a sync Bevy schedule — systems can't `.await`. Driving an async fn from the render schedule means leaving the schedule to an external pool, then polling the resulting `Task<...>` from a separate sync system in `ExtractSchedule`. That's a state machine with a different surface, not a simpler primitive. (ii) Same caveat as Q2 (wasm AsyncComputeTaskPool is main-thread-only — but here it doesn't matter because the work is "wait for GPU mapping" not "do CPU work", so `spawn_local` is fine). (iii) The wgpu canonical pattern for cross-target readback is the state machine + AtomicBool callback; e.g. wgpu examples' `capture` sample uses exactly this pattern (`render_device.poll(Poll)` + flag + `get_mapped_range`).
   - **Rejected option:** "Skip on web" (the current interim hack). **Rejected per Decision 2** — non-negotiable.
   - **Deciding factor:** The cross-frame state machine is unmodified between targets — same code on native and WebGPU — and only requires `Arc<AtomicBool>` (std-only). The `wait_indefinitely`-replaces-with-poll change is mechanical.

4. **Q5 (tracing error counter): custom `tracing` Layer chosen, `PipelineScanResult` widening rejected, scoping to pipeline errors rejected.**
   - **Rejected option:** Widen `PipelineScanResult` (`e2e/checks.rs:44`) to also count tracing-level errors. **Rejected because:** `PipelineScanResult` is specifically about `bevy_render::PipelineCache` errors (shader compile, bind-group validation) caught by the render-world scan system at `e2e/checks.rs:60`. Folding `tracing::error!` into it loses the diagnostic distinction the existing gate plumbing depends on (`PipelineScanResult::0` returns an `Option<Result<(), String>>` representing pipeline-cache-error specifically — see `e2e/checks.rs:132-142`). The two signals are independent; merging is a code smell.
   - **Rejected option:** Scope to "pipeline errors only" (don't add a tracing-level counter at all). **Rejected because:** The handoff at `/tmp/web-vox-async-loading-handoff.md:213-217` explicitly says "Asserts no `tracing::error!` calls fired during the run". The brief is binding.
   - **Deciding factor:** `tracing-subscriber::Layer` is the canonical hook for level-based event counting. Bevy 0.19's `LogPlugin::custom_layer` (or `log_plugin_layers`) exposes the registration site cleanly — see [Assumptions](#assumptions-made) §6 for the API verification step.

5. **Q5 (skybox-empty mechanism): new `GridPreset::Empty` chosen, new `AppArgs.skybox_only_phase: bool` rejected.**
   - **Rejected option:** A new bool field `AppArgs.skybox_only_phase` that short-circuits `setup_test_grid`'s match arms. **Rejected because:** (i) `GridPreset` is the existing first-class "what world to load" enum (`lib.rs:65-78`); adding bool side-channels next to it creates two sources of truth for scene selection. (ii) A `GridPreset::Empty` variant naturally extends to the web side (the `?skybox=1` query string in Q6 maps to "install empty world", same semantic). (iii) Existing pattern: each prior gate added a `*_mode: bool` to `AppArgs` for things that orthogonally modify the e2e flow (`oasis_edit_visual_mode`, `vox_gpu_oracle_cpu_phase`); but those are **flow modes**, not scene-selectors. `GridPreset` is the right axis.
   - **Deciding factor:** Scene selection lives on `GridPreset`. Test-mode flags live on `AppArgs.*_phase: bool`. Mixing them blurs intent. Adding the third variant is one enum row + one match arm in `setup_test_grid`.

6. **Q6 (Playwright SSIM): `--ssim-compare` flag on `e2e_render` chosen, dedicated `e2e_image_compare` bin rejected.**
   - **Rejected option:** New dedicated bin `crates/bevy_naadf/src/bin/e2e_image_compare.rs`. **Rejected because:** (i) `e2e_render` already shells out to itself for the two-phase oracle pattern (`vox_gpu_oracle.rs:372,400`). The binary is the "every e2e thing" entry point. (ii) The SSIM body is ~30 lines once you reuse `vox_gpu_oracle.rs:471-582`'s `compare_oracle_frames` body shape (it's basically two `load_png_as_framebuffer` + one `rgb_similarity_structure` + one comparator); adding a whole new bin just to host 30 lines triples Cargo.toml + manifest cost. (iii) Existing precedent: `bin/e2e_render.rs` already has CLI-flag short-circuits for non-app-booting modes (`--validate-gpu-construction-scaled` at :151-161, `--validate-gpu-construction-production` at :167-177, `--vox-gpu-oracle` top-level at :142-145).
   - **Deciding factor:** `e2e_render` is the established "everything e2e" entry point. Adding a flag is uniform with the existing pattern.

7. **Q6 (skybox baseline mechanism on web): `?skybox=1` query string chosen, reusing Q5's `GridPreset::Empty` directly rejected.**
   - **Rejected option:** Have the wasm wire receive a `GridPreset::Empty` somehow (e.g. via a URL-encoded `?preset=empty`). **Rejected because:** the web `AppArgs` is always `AppArgs::default()` (no CLI flags on web); plumbing GridPreset variants through the URL would mean serialising the enum or adding a separate string-to-preset parser on the web side. The bool `?skybox=1` is the simpler boundary — the wasm reads it once and dispatches into a one-line `install_empty_world` call before `setup_test_grid` runs. Server-side Playwright sees the bool, client-side wasm acts on the bool.
   - **Deciding factor:** Smallest possible URL surface. The bool is sufficient because skybox-only is the only "other than default" web preset the gate needs.

8. **Q6 (binary-or-flag): single binary call chosen, in-process Node SSIM lib rejected.**
   - **Rejected option:** Add a Node SSIM library (`image-ssim` or `ssim.js`) to `e2e/package.json` and run the compare in Playwright. **Rejected because:** Decision 4 in 01-context.md: "zero metric drift — same SSIM impl on both sides". A different Node lib would drift from the Rust gate (different windowing, different luminance conversion, different gamma assumptions). Shelling out to the same `image-compare` crate eliminates the divergence axis entirely.
   - **Deciding factor:** Decision 4 (binding).

9. **Q5 vs Q6 sharing the SSIM comparator.** Both gates use the same
   `image_compare::Algorithm::MSSIMSimple` call site — Q5 inline in the
   gate's `compare_dissimilar_frames`, Q6 via `e2e_render --ssim-compare`.
   Per Decision 4 (zero metric drift) the implementer factors out a
   single helper function (could live in `framebuffer.rs` or a new
   `e2e/ssim.rs`) that takes `(&Framebuffer, &Framebuffer) -> Result<f64, String>`
   and is called from both call sites.

## Assumptions made

1. **`bevy::tasks::Task<T>` works uniformly on native + web.** On native, `AsyncComputeTaskPool::spawn` returns a `Task<T>` that runs on a worker thread. On wasm, the same call returns a `Task<T>` that runs on the main thread (`single_threaded_task_pool.rs:184-200`). Both expose `poll_once` and both behave like a `Future`. The polling system at Q2 uses the same `block_on(poll_once(task))` shape on both. The **difference** is whether the work actually runs off-thread: native yes, web no. For the **parse** work on web we route via `wasm-bindgen-rayon` instead; the `PendingVoxParse` resource on web carries a `crossbeam_channel::Receiver` instead of a `Task`. The implementer's polling system is cfg-gated:
   ```rust
   #[cfg(not(target_arch = "wasm32"))]
   pub struct PendingVoxParse { task: Option<bevy::tasks::Task<...>> }
   #[cfg(target_arch = "wasm32")]
   pub struct PendingVoxParse { rx: crossbeam_channel::Receiver<...> }
   ```
   Two polling systems (cfg-gated), one resource name. **Implementer re-verifies** this is the cleanest split during impl — alternative is to wrap both in a `dyn TaskLike` trait but that's overkill.

2. **The `bevy_pixel_world` build config is genuinely proven, but its runtime layer no longer exercises `wasm-bindgen-rayon`.** History per user clarification (2026-05-18): `wasm-bindgen-rayon` was the original web threading approach in `bevy_pixel_world` and **was proven working** with the exact build configuration at `.cargo/config.toml` (the `+atomics,+bulk-memory,+mutable-globals,+shared-memory` rustflags + 1GB max-memory + `--export=__wasm_init_tls`/`__tls_size` link-args + nightly `build-std`). The runtime wiring was later replaced by an emscripten-packed-into-WebWorker pattern (`workers/` + the `sim2d_noise` C library via `Trunk.toml` pre-build hook) **specifically because that C library couldn't be packed into idiomatic Rust threading** — not because `wasm-bindgen-rayon` itself failed. Current source confirms only the build-config residue: `grep -rn "wasm-bindgen-rayon\|init_thread_pool"` against the entire bevy_pixel_world tree returns ONLY the comment in `.cargo/config.toml:12`. **Implication for bevy-naadf:** copy the build configuration verbatim (it works) and wire the runtime layer fresh from `/tmp/wasm-bindgen-rayon-1.3.0/README.md:44-80`. No second-guessing the build flags — they're load-bearing residue from a working integration. Verified directly against the crates.io cache extract at `/tmp/wasm-bindgen-rayon-1.3.0/` during this design phase.

3. **`AsyncComputeTaskPool::get()` returns the actual `TaskPool` on native + wasm.** On native, Bevy installs the global `AsyncComputeTaskPool` during `DefaultPlugins` boot via `TaskPoolPlugin` (the standard machinery). On wasm, the same plugin runs and installs a `single_threaded_task_pool::TaskPool` so `AsyncComputeTaskPool::get()` returns a valid pool that schedules onto `spawn_local`. **Implementer re-verifies** by adding a `bevy::tasks::AsyncComputeTaskPool::try_get().is_some()` probe in `Startup` on both targets; if either returns `None`, the install path falls back to sync (existing pattern at `world/data.rs:811-813` is the model).

4. **wgpu 25.x exposes `Buffer::map_state()` OR the AtomicBool-from-callback pattern works on Bevy 0.19's `bevy::render::render_resource::Buffer` re-export.** The Bevy `Buffer` type wraps `wgpu::Buffer` but historically does not expose all wgpu surface area. **Implementer verifies during impl** which of two paths works:
   - Path A (preferred): call `buffer.deref().map_state()` if the inner wgpu buffer is publicly reachable via `Deref` on the Bevy wrapper (Bevy 0.19 typically exposes this).
   - Path B (fallback): use the `Arc<AtomicBool>` set inside the `map_async` callback closure, polled per render frame. This is the wgpu-cookbook-canonical pattern and works on every wgpu API revision.
   If Path A is available the code is shorter; otherwise Path B. Either way, no design change is required — the state machine is the same.

5. **The `VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX = 0.85` threshold is a starting estimate.** Per the brief: tune empirically during impl. Expected: a correctly-loaded `.vox` rendered at `ORACLE_CAMERA_POS (744, 800, 672)` looking at `(744, 100, 672)` (high downward view of populated Oasis voxels) produces a frame with substantial dark voxel content and shadow gradients, structurally very different from the pure sky baseline. The actual SSIM should land between 0.2 and 0.6 (well below 0.85), but the implementer empirically measures during impl by:
   - Run `--vox-web-parity-skybox` once → save baseline.
   - Run `--vox-web-parity-loaded` once → save loaded.
   - Run `--ssim-compare baseline loaded` once → record score.
   - Set `VOX_WEB_PARITY_SSIM_DISSIMILARITY_MAX = round_up(measured + 0.10, 2)` so e.g. measured 0.45 → threshold 0.55. If the measured value lands above 0.85 the test is wrong — likely the camera is framing the sky portion of the loaded world rather than the geometry; re-pin the camera lower (Y=200 instead of Y=800) until the geometry dominates.

6. **`bevy_log::LogPlugin` in Bevy 0.19 exposes a `custom_layer` registration hook.** The exact field name may be `custom_layer: Option<fn(&mut App) -> Option<BoxedLayer>>` or `log_plugin_layers: Vec<BoxedLayer>` or `custom_subscriber_layer: Option<...>` — Bevy has changed this several times across versions. **Implementer verifies the exact API during impl** by:
   - `grep -n "custom_layer\|custom_subscriber" $(cargo metadata --format-version 1 | jq -r '.packages[] | select(.name=="bevy_log") | .manifest_path' | xargs dirname)/src/lib.rs`
   - If the hook field has a different name, adapt. If no hook exists at all (a Bevy version regression), the implementer falls back to a manual `tracing_subscriber::registry().with(CountingLayer(...)).init()` call **before** `DefaultPlugins` — but this requires disabling Bevy's own `LogPlugin` (set `.disable::<LogPlugin>()`), which loses Bevy's own logger setup. Path A (hook field) is strongly preferred.

7. **The `_headers` file + `serve.mjs` mirror correctly.** Verified at `crates/bevy_naadf/_headers:7-9` (`Cross-Origin-Opener-Policy: same-origin` + `Cross-Origin-Embedder-Policy: require-corp`) and `e2e/serve.mjs:46-48` (same two headers). `SharedArrayBuffer` is reachable in both Cloudflare-deployed production AND Playwright-local-served. **Implementer re-verifies** during impl by `console.log(crossOriginIsolated)` in the wasm-init shim — if false, the headers aren't applied (Chrome dev tools also show this under Application → Frames).

8. **`wasm-bindgen-rayon` works with Trunk 0.21's default bundler-style asset linkage.** Trunk 0.21 uses `--target web` for the wasm-bindgen output (default). `wasm-bindgen-rayon`'s docs (`/tmp/wasm-bindgen-rayon-1.3.0/README.md:97`) require `--target web`. The crate's `workerHelpers.js` uses `new URL(import.meta.url)` / `new Worker(...)` which Trunk's HTML asset pipeline doesn't pre-process — but the JS is served as-is from the wasm-bindgen output dir, and `wasm-bindgen-rayon` is designed to work with bundlers OR direct serving (without the `no-bundler` feature, the JS uses module-relative paths that Trunk's dev server resolves correctly). **Implementer re-verifies** during impl: if Trunk's output produces a 404 on `workerHelpers.js`, switch the `wasm-bindgen-rayon` feature to `no-bundler` (`wasm-bindgen-rayon = { version = "1.3", features = ["no-bundler"] }`). Both work; no-bundler imposes the additional requirement that the wasm module's URL be passed via `pool.mainJS()` from the bootstrap — a 3-line shim addition.

9. **Bevy 0.19's `Buffer::label()` method exists.** wgpu 25 has it; Bevy's re-export typically passes through. If not, the implementer falls back to stashing labels in a parallel `HashMap<BufferId, &'static str>` on `ConstructionGpu`. The assertion (Q4) is debug-only; failure to compile on a Bevy regression is a clear signal.

10. **The PNG capture from Playwright matches `Framebuffer::from_image`'s RGBA encoding.** Playwright's `canvas.screenshot()` returns a PNG which, when decoded via `image::open` + `to_rgba8`, produces a buffer compatible with `Framebuffer::from_raw_rgba` (used by `load_png_as_framebuffer` at `vox_gpu_oracle.rs:616-631`). Both halves use the `image` crate's PNG decoder so the result is bit-identical regardless of capture source. **Implementer verifies** during impl by capturing both a `--vox-web-parity-skybox` PNG (Rust-side) and a `canvas.screenshot()` skybox PNG (Playwright) of the SAME wasm-rendered frame and visually diffing. If they differ structurally, capture path differs (alpha channel, gamma) and the implementer either matches the post-processing on both sides or moves the SSIM comparison to a shared input format.

## Implementer ordering (recommended)

The implementer applies changes in this order so each step is verifiable
in isolation:

1. **Foundation deps + toolchain.** Edit `rust-toolchain.toml`; add
   `.cargo/config.toml`; add `wasm-bindgen-rayon`/`rayon`/`crossbeam-channel`
   to `crates/bevy_naadf/Cargo.toml`. Verify `cargo build --workspace`
   green (still using sync `.vox` paths everywhere; nothing breaks).

2. **Q1 part 1 — JS bootstrap.** Patch `index.html` + `init.js.template`
   to call `initThreadPool` after `init`. Re-export `init_thread_pool`
   from `web_vox.rs`. `cargo build --target wasm32-unknown-unknown --bin
   bevy-naadf --no-default-features --features webgpu` + `trunk build` + manual
   `crossOriginIsolated === true` check in dev console.

3. **Q1 part 2 + Q2 — refactor `install_vox_bytes_in_fixed_world` into
   parse/install halves.** Split `grid.rs:325-450` into
   `parse_to_imported_vox` + `install_imported_vox`. Keep the public
   sync wrapper. `cargo build --workspace` green.

4. **Q2 — native AsyncComputeTaskPool spawn + poll-in-Update.** Add
   `PendingVoxParse` resource, polling system, register in
   `crate::build_app`. Rewire `setup_test_grid::GridPreset::Vox` arm and
   `native_vox_drop_listener` to use `spawn_native_vox_parse`. `cargo run
   --bin e2e_render -- --vox-e2e` (the existing native gate that loads a
   synthesised `.vox` through the same path) green.

5. **Q1 part 3 — wasm rayon parse pump.** Wire `spawn_parse_task` +
   `PARSE_RESULT_RX` into `apply_pending_vox`. `trunk build` green; `just
   test-wasm` (existing red spec) advances from "panic at readback" to
   "the parse no longer freezes the UI; the panic at readback is still
   there" — the Q3 work follows in step 6.

6. **Q3 + Q7 — cross-frame readback state machine + delete interim hack.**
   Refactor `populate_cpu_mirror_from_gpu_producer` (`mod.rs:897-1060`).
   Delete the wasm32 escape hatch at `mod.rs:944-957`. `cargo run --bin
   e2e_render -- --vox-gpu-oracle` (existing native oracle gate that
   exercises the readback heavily) green; `just test-wasm` now green
   on the previously-red spec.

7. **Q4 — confirmation assertion.** Add the debug-mode assertion to
   `populate_cpu_mirror_from_gpu_producer`. `cargo build --workspace`
   green; `cargo test --workspace --lib` green.

8. **Q5 — new native gate `--vox-web-parity`.** Add
   `GridPreset::Empty` + `install_empty_world` + `vox_web_parity.rs`
   module + driver phases + flag wiring + tracing-error counter
   + custom `LogPlugin` layer registration. `cargo run --bin e2e_render
   -- --vox-web-parity-skybox` green (saves PNG); `cargo run --bin
   e2e_render -- --vox-web-parity-loaded` green (saves PNG); `cargo run
   --bin e2e_render -- --vox-web-parity` green (compares; threshold
   tuned per [Assumptions](#assumptions-made) §5).

9. **Q6 — `--ssim-compare` flag + Playwright spec extension.** Add the
   short-circuit to `bin/e2e_render.rs`; factor out
   `load_png_as_framebuffer` to a shared location; add `ssim_compare_command`.
   Add `?skybox=1` query support to `web_vox.rs` + `WebSkyboxOverride`
   resource + `setup_test_grid` check. Extend
   `e2e/tests/vox-loading.spec.ts` with the skybox-baseline run + the
   `child_process.spawn` SSIM compare. `just test-wasm` green; PNGs
   attached to Playwright report show real dissimilarity.

After step 9, run the regression checks per the handoff
(`/tmp/web-vox-async-loading-handoff.md:334-345`):
`--vox-e2e`, `--oasis-edit-visual` both green, `cargo test --workspace
--lib` green, `cargo build --target wasm32-unknown-unknown` green.
