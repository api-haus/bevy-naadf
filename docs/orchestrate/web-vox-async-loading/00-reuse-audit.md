# Re-implementation audit — web-vox-async-loading
2026-05-18

## Summary
- 8 high-confidence reuse candidates (helpers, infrastructure, existing call patterns)
- 4 partial reuses (need extension)
- 2 borderline calls (depend on architectural choices in Q1/Q2/Q3)
- No prior orchestration tackled async parse / async GPU readback — `streaming-world` and `web-chunks-storage-buffer` predecessor work touched buffer layouts and procedural noise but not async lifecycle on either target.

## Candidates table

| # | Existing artefact (file:lines) | Covers | Reuse-as-is / extend / inspire | Confidence | Notes |
|---|--------------------------------|--------|--------------------------------|------------|-------|
| 1 | `crates/bevy_naadf/src/voxel/web_vox.rs:34-72,184-292,295-338` | Q1 web inbox + dnd shim + two-stage deferred parse + overlay helpers | **extend** | high | `PENDING_VOX_BYTES` / `QUEUED_FOR_INSTALL` inbox + `apply_pending_vox` is the seam to extend. Stage 2's *sync* install is the bottleneck — replace its body with a Task-poll pattern; the inbox + overlay code remains untouched. |
| 2 | `crates/bevy_naadf/src/voxel/vox_import.rs:154-225` (`parse_vox_bytes`, `parse_dot_vox_data`, `parse_dot_vox_data_tiled`) | Q1+Q2 reusable pure-CPU bytes→`ImportedVox` parse | **reuse-as-is** | high | Pure / no Bevy / no fs. Drop-in body for whichever async task wrapper the architect chooses (worker, AsyncComputeTaskPool, JS WebWorker). Already used by both native (`load_vox`) and web. |
| 3 | `crates/bevy_naadf/src/voxel/grid.rs:325-450` (`install_vox_bytes_in_fixed_world`) | Q1+Q2 Bevy-resource install (camera pose, ModelData, WorldData) | **reuse-as-is** | high | Already split out of native path. Takes `&[u8]` + `&str`. Both `apply_pending_vox` and `native_vox_drop_listener` call it. The only thing the new async path adds is producing the `ImportedVox` off-thread. |
| 4 | `crates/bevy_naadf/src/world/data.rs:806-820` (`bevy::tasks::ComputeTaskPool` poll-or-fallback pattern) | Q2 task-pool integration pattern | **inspire** | high | Existing template for `ComputeTaskPool::try_get().is_some()` + fallback. Same shape works for `AsyncComputeTaskPool` (the natural choice for the parse-blocking-IO case) — copy the guard pattern. |
| 5 | `crates/bevy_naadf/src/e2e/vox_gpu_oracle.rs` (entire file) | Q5 SSIM-compare native gate template | **reuse-as-is** for helpers / **extend** for the new gate | high | Already wires the harness via `crate::AppArgs` flags, framebuffer capture, PNG save, dual-process spawn, image-compare MSSIMSimple, sanity guards (brightness floor/dark floor, mean delta, dimensions). The new gate copies the structure with two phases — skybox-empty vs vox-loaded — and assert *dissimilar* (SSIM < threshold) rather than similar. Helpers `load_png_as_framebuffer`, `framebuffer_to_rgb_image`, `oracle_cpu_png_path`/`oracle_gpu_png_path`, `save_oracle_screenshot`, `count_pixels_with_luminance_above` are all reusable verbatim. |
| 6 | `crates/bevy_naadf/src/e2e/framebuffer.rs:207` (`from_raw_rgba`), `save_png`, `mean_pixel_delta`, `region_mean`, `Framebuffer::luminance` | Q5 framebuffer wrapper + PNG IO | **reuse-as-is** | high | Used by every existing gate; new gate just calls the same surface. |
| 7 | `crates/bevy_naadf/src/e2e/mod.rs:200-279` (`add_e2e_systems`), `crates/bevy_naadf/src/e2e/driver.rs:200-247` (`E2ePhase` extensions, `E2eOutcome`, screenshot drain phases) | Q5 wiring (camera pin, screenshot capture phases, AppExit, pipeline-error scan, error-counter — `E2eOutcome::gate_result`) | **extend** | high | The existing `oasis_edit_visual` and `vox_gpu_oracle` modes both register `(state resource, pin_*_camera system, driver phase-branch enum variants)` triples — the new `--vox-web-parity` gate follows the same pattern. Pipeline-error scan via `PipelineScanResult` is already cross-world and folds into `AppExit::error()` — re-use. |
| 8 | `crates/bevy_naadf/src/e2e/vox_e2e.rs:325-369` (`run_vox_e2e` + `assert_vox_geometry_visible`) | Q5 reference for "non-skybox" assertion shape | **inspire** | medium | A luminance-floor "non-skybox" gate already exists. The new gate prefers SSIM dissimilarity (per handoff), but `vox_e2e`'s pattern — boot harness with `AppArgs.grid_preset = Vox`, custom mode flag, custom driver-phase fork — is the template. |
| 9 | `crates/bevy_naadf/src/render/construction/mod.rs:1199-1268,1561-1581` (production `naadf_*_gpu_producer` and `naadf_segment_voxel_buffer_w5` buffers) | Q4 the COPY_SRC status of every production-path source buffer | **reuse-as-is** | high | Production-path buffers already have correct flags: `hash_map_gpu_producer` (COPY_SRC ✓), `hash_coefficients_gpu_producer` (no COPY_SRC — never read back), `block_voxel_count_gpu_producer` (COPY_SRC ✓), `segment_voxel_buffer_w5` (COPY_SRC ✓). The W2 *placeholder* block at :1916-1960 only fires when `want_gpu_producer = false` (no `ModelData` and no `dense_voxel_types`), so the three flagless placeholders are never touched on the production .vox path. See Q4 below. |
| 10 | `crates/bevy_naadf/index.html:23-189`, `init.js.template:17-22` | Q1 loading-overlay DOM API (`window.hideLoading`, `window.updateLoadingProgress(loaded, total)`, `#progress-fill.indeterminate`, `#progress-text`) + JS streaming-fetch template (ReadableStream + getReader().read() loop) | **reuse-as-is** | high | The new async pipeline must drive these — do not invent a parallel overlay. `init.js.template`'s wasm-streaming loop is the canonical example for "fetch with progress". |
| 11 | `crates/bevy_naadf/_headers:8-9` (COOP/COEP: same-origin / require-corp) | Q1 cross-origin-isolation already enabled | **reuse-as-is** | high | `SharedArrayBuffer` is enabled today. `wasm-bindgen-rayon` would work without header changes. The CI deploys these headers via `_headers` (Cloudflare Pages). The local Playwright server (`e2e/serve.mjs:46-48`) mirrors them. |
| 12 | `crates/bevy_naadf/src/world/buffer.rs:33-39` (`GROWABLE_BUFFER_USAGES = STORAGE \| COPY_SRC \| COPY_DST`) | Q4 confirms `WorldGpu::blocks` and `WorldGpu::voxels` already support readback | **reuse-as-is** | high | Matches the handoff claim. Also `prepare.rs:284-292` `naadf_chunks` buffer is `STORAGE \| COPY_DST \| COPY_SRC`. |
| 13 | `crates/bevy_naadf/src/e2e/checks.rs` + `crates/bevy_naadf/src/e2e/mod.rs:269-278` (`PipelineScanResult` + render-world scan) | Q5 the "no `tracing::error!`-equivalent" pipeline-error counter | **reuse-as-is** | medium | Existing scan already folds pipeline errors into the `AppExit`. The handoff also asks for "zero `tracing::error!` calls" specifically — that's NOT what `PipelineScanResult` counts. **No existing tracing-level error counter** in `src/e2e/` — that has to be added (borderline call 1 below). |

## Per-question coverage map (Q1–Q7)

### Q1 (web async parse)
- **Reusable infrastructure:**
  - The single-slot inbox + two-stage deferred-parse driver in `voxel/web_vox.rs` is the seam. Replace `apply_pending_vox`'s stage-2 sync install with a Task-poll pattern; leave stage-1 and the inbox alone.
  - `wasm_bindgen_futures::spawn_local` is already a direct dep (`crates/bevy_naadf/Cargo.toml:126-128`) and is used twice in `web_vox.rs` (fetch + dnd buffer read).
  - `parse_vox_bytes` is pure CPU and trivially callable from a worker / off-main-thread context.
  - Cross-origin isolation (COOP/COEP) is already on (`_headers:8-9`; `serve.mjs:46-48`). `SharedArrayBuffer` works → `wasm-bindgen-rayon` works.
  - The DOM overlay (`index.html`) already exposes `#loading.hidden` + `window.hideLoading()` + `window.updateLoadingProgress(loaded, total)` + `.indeterminate` mode. The Rust side already toggles `.hidden` (`web_vox.rs:90-119`). The streaming-fetch pattern is documented in `init.js.template`.
- **Missing (the architect must choose between):**
  - `wasm-bindgen-rayon` is NOT a dep today. Adding it is greenfield wiring but the platform (headers, wasm-bindgen) is ready.
  - `bevy::tasks::AsyncComputeTaskPool` is NOT used anywhere on web. Whether its wasm32 backend is a real worker or a `spawn_local` shim must be answered by the architect (handoff Q1 explicitly says "verify whether the wasm32 build of `bevy_tasks` has a real worker backend or is just sugar over `spawn_local`"). Not greenfield, but verification is needed.
  - A JS WebWorker shim (worker.js that hosts a wasm instance + posts results back) is NOT present. The `workers/` directory at the worktree root is the Cloudflare Workers (R2 proxy), unrelated.

### Q2 (native async parse)
- **Reusable infrastructure:**
  - `parse_vox_bytes` already returns an `ImportedVox` from a `&[u8]` — pure CPU, no Bevy, no fs. Trivially `Send`.
  - `install_vox_bytes_in_fixed_world` already takes `&[u8]` and was split out of `install_vox_in_fixed_world` exactly so multiple entry points (native fs read, web HTTP fetch, dnd on both targets) share one install pass. Either Bevy `AssetLoader` or `AsyncComputeTaskPool::spawn(...)` can feed bytes into it.
  - `bevy::tasks::ComputeTaskPool::get()` pattern at `world/data.rs:811-813` is the inspiration for the wgpu-side analog — `AsyncComputeTaskPool` follows the same API shape.
- **Existing `AssetLoader` registrations to model on:** `crates/bevy-instamat/src/baked_material.rs:215-223` (`register_asset_loader(MaterialRonLoader)`) and `crates/bevy_naadf/src/texture_array/{loader,mod}.rs` (`register_asset_loader(TextureArrayLoader)`). Both implement `bevy::asset::AssetLoader::load(...)`. No `*.vox` `AssetLoader` exists. The architect chooses between an `AssetLoader<ImportedVox>` (Bevy-natural) and an explicit `AsyncComputeTaskPool::spawn` + poll-in-`Update` pattern.
- **Drag-drop entry point:** `voxel/grid.rs:471-529` (`native_vox_drop_listener`) already gets `Commands` and the bytes via `std::fs::read`. The new async path needs to either (a) hand the bytes/path to the same async task pump used by startup, or (b) own its own pump. The single-resource Task<ImportedVox> pattern handles both.

### Q3 (async GPU readback)
- **Reusable infrastructure:**
  - **Every** existing `map_async` call site in the worktree uses the *blocking* pattern `slice.map_async(...); device.poll(PollType::wait_indefinitely()).unwrap(); slice.get_mapped_range()`. No existing async-readback / cross-frame state machine / oneshot-channel pattern exists in this codebase. Sites grepped: `world/buffer.rs:286-291`, `render/construction/mod.rs:983, 3339, 3372, 3402, 4576, 4609, 4641, 5062, 5095, 5120, 5597, 5630, 5654, 5956, 6338, 6370, 6394, 6884, 6916, 6945, 6968, 7815, 8151, 8188, 9141, 9197`, `render/construction/world_change.rs:567, 599`, `render/construction/bounds_calc/tests.rs:319, 350`. Of these, **only line 983 is on the production runtime path** (`populate_cpu_mirror_from_gpu_producer`); every other site is a test / `validate_*` / diagnostic helper.
  - `crossbeam-channel`, `futures-channel`, and `futures-util`/oneshot machinery are already transitive deps in `Cargo.lock` — no new crate is needed for a oneshot channel pattern.
  - `bevy::tasks::AsyncComputeTaskPool` is available natively (used by `bevy_tasks` already). On wasm `bevy::tasks::block_on` is used in `bake.rs:34` (native-only binary, not relevant) — but the same crate exposes `Task` / `spawn` / `poll`.
- **Missing:** a callbacks-driven readback helper, or a `Task<Vec<u32>>` produced by `map_async` + oneshot. Greenfield, but the wiring is straightforward.

### Q4 (other W2 placeholders' COPY_SRC)
The handoff is correct that the W2 placeholders lack COPY_SRC on three of the four buffers, but **none of them are on the production .vox path**. The .vox path triggers `want_gpu_producer = true` (because `model_data` is `Some` per the gate-widening at `render/construction/mod.rs:1184-1186`), which routes allocation through the `naadf_*_gpu_producer` block at :1187-1323, and additionally the W5 `naadf_segment_voxel_buffer_w5` block at :1561-1581. The flagless W2 placeholders at :1916-1960 only allocate when `gpu.<buffer>.is_none()` AND no other block allocated them earlier in this `prepare_construction` call — which in production happens only on non-VOX runs without `dense_voxel_types`.

Production-path source buffers and their COPY_SRC status (verified against current worktree):

| Buffer (label) | Site | COPY_SRC? | Read back today? |
|----------------|------|-----------|------------------|
| `naadf_hash_map_gpu_producer` | mod.rs:1199-1227 | yes (:1203) | no |
| `naadf_hash_coefficients_gpu_producer` | mod.rs:1233-1240 | **no** (:1236) | no |
| `naadf_block_voxel_count_gpu_producer` | mod.rs:1260-1267 | yes (:1263) | yes (production readback at :993) |
| `naadf_segment_voxel_buffer_gpu_producer` (dense path, not VOX) | mod.rs:1312-1319 | **no** (:1315) | no |
| `naadf_segment_voxel_buffer_w5` (VOX path) | mod.rs:1569-1577 | yes (:1574) | no |
| `naadf_model_data_*_buffer` | created via `generator_model::create_storage_buffer_u32` | TBD — see Q4 borderline | no (host-uploaded read-only) |
| `naadf_chunks` (WorldGpu) | prepare.rs:284-292 | yes (:291) | yes (production readback at :1010) |
| `WorldGpu.blocks/voxels` (GrowableBuffer) | buffer.rs:33-39 | yes | yes (production readback at :1027-1028) |

Three flagless W2 placeholders (`segment_voxel_buffer_w2_placeholder`, `hash_map_w2_placeholder`, `hash_coefficients_w2_placeholder`) are dead on the .vox path. If the readback path ever needs to read them back too (it doesn't today), adding COPY_SRC is a one-line addition per buffer. **No production-path source buffer that `populate_cpu_mirror_from_gpu_producer` reads from is missing COPY_SRC.** The handoff phrasing ("audit `naadf_*_w2_placeholder` buffers for missing COPY_SRC after the async readback path lands") should be interpreted as "after the async readback lands, re-confirm the placeholders are still untouched on .vox runs" — which they will be, since the same gate at :1184-1186 routes the allocation.

The ~40 `copy_buffer_to_buffer` hits in `mod.rs` break down to:
- production-path hits: 980 (the readback in `populate_cpu_mirror_from_gpu_producer`) — the only production-runtime copy.
- test/diagnostic hits: all of 3336, 3369, 3399, 4573, 4606, 4638, 5059, 5092, 5117, 5594, 5627, 5651, 5953, 6335, 6367, 6391, 6881, 6913, 6942, 6965, 7812, 8148, 8185, 9138, 9194 — these live inside `validate_gpu_construction*`, `run_one_*_byte_diff`, `run_oasis_segment_byte_diff`, and other diagnostic helpers used only by `--validate-gpu-construction*` and `--vox-e2e`-adjacent flags. None touch the `*_w2_placeholder` buffers (verified by label inspection).

### Q5 (new native e2e gate)
Reuse `vox_gpu_oracle.rs` as the template — handoff is explicit. Specific reusable pieces:

- **SSIM compare** — `compare_oracle_frames` (vox_gpu_oracle.rs:471-582), `framebuffer_to_rgb_image` (:603-612), `load_png_as_framebuffer` (:616-631), `image_compare::Algorithm::MSSIMSimple` integration. The new gate's compare phase shape: **(a) load skybox-baseline.png + vox-loaded.png from disk; (b) compute MSSIMSimple; (c) assert `score < threshold`** (note inversion — vox_gpu_oracle asserts similarity, the new gate asserts dissimilarity).
- **Subprocess two-phase orchestration** — `run_vox_gpu_oracle_compare` (:346-463) shows the pattern: `std::process::Command::new(current_exe()).arg("--vox-gpu-oracle-cpu").status()` then `.arg("--vox-gpu-oracle-gpu")`. The new gate uses the same shape — `--vox-web-parity-skybox` and `--vox-web-parity-loaded` sub-modes plus the top-level `--vox-web-parity` that spawns them and compares.
- **Camera pinning** — `pin_vox_gpu_oracle_camera` (:642-659) shows the gate-specific pose-pin pattern; the new gate copies it.
- **State stash** — `VoxGpuOracleState` (:669-672); copy the pattern: `Resource` with `Option<Framebuffer>` + `bool saved`.
- **PNG save** — `save_oracle_screenshot` (:675-686). Reuse directly with new filenames.
- **Sanity guards** — brightness floor + dark floor (`count_pixels_with_luminance_above`, `count_pixels_with_luminance_below`) defend against degenerate captures (pure sky / pure black). The new gate inverts the application — the skybox baseline should be mostly sky-tinted (high dark-pixel count low; assert the *skybox* frame *is* sky-tinted with high mean luminance OR just confirm pipeline scan passed).
- **Harness flags** — `AppArgs.vox_gpu_oracle_cpu_phase` and `_gpu_phase` (lib.rs:385-394) show the per-mode-flag pattern. Add `vox_web_parity_skybox_phase` and `vox_web_parity_loaded_phase` analogues.
- **`add_e2e_systems` registration** — mod.rs:228 (`init_resource::<vox_gpu_oracle::VoxGpuOracleState>()`) and :260-261 (`pin_vox_gpu_oracle_camera.after(pin_oasis_camera)`). Same pattern for the new gate.
- **Framebuffer-PNG capture** — `crates/bevy_naadf/src/e2e/readback.rs` + the standard `Screenshot::primary_window()` capture flow in `driver.rs` is already shared by all gates.
- **AppExit + outcome** — `E2eOutcome` (`driver.rs:243-247`) is the `Option<Result<(),String>>` slot every gate writes its verdict into; the runner reads it via `app_exit`. The new gate writes here.

**Missing for Q5 (must be added):**
- A way to boot the harness in "skybox-empty" mode. The handoff suggests a new `GridPreset::Empty` variant. Alternative: a new `AppArgs.skybox_only_phase: bool` flag that short-circuits `setup_test_grid`'s install branches.
- A `tracing::error!` call counter. `PipelineScanResult` counts pipeline errors specifically; it does *not* count tracing-level errors. This is a real gap — see borderline call 1.
- A `bool` flag (`AppArgs.vox_web_parity_*`) wired into the driver state machine.

### Q6 (Playwright SSIM)
- **No Node SSIM/pixelmatch helper exists.** `e2e/package.json:9-11` shows only `@playwright/test`. `e2e/tests/helpers/` only has `console-collector.ts`.
- **Existing pieces to reuse:** `vox-loading.spec.ts:135-148` already shows `canvas.screenshot()` → PNG bytes → `test.info().attach(...)` → `fs.writeFile(...)`. The capture mechanism is solved; only the SSIM compare itself is missing.
- **The architect picks one of:**
  - Shell out to a tiny Rust binary (could be a new bin like `e2e_image_compare`, or expose `compare_oracle_frames` via an existing binary's `--ssim-compare <a.png> <b.png>` flag). Reuses `image-compare` already vendored. Bonus: same SSIM impl on both sides → no Node/Rust SSIM-impl drift.
  - Add a Node SSIM/pixelmatch dep (`pixelmatch` does pixel diff, not SSIM; `image-ssim` / `ssim.js` do SSIM). Greenfield Node code path; risk of metric divergence vs the Rust gate.
- **Loading-overlay hooks reusable for "wait for skybox baseline":** `#loading.hidden` already signals "wasm booted". The new spec will need a way to *suppress* the .vox fetch for the skybox baseline run — either a `?skybox=1` query string (mirrors the existing `?vox=<url>` override pattern at `web_vox.rs:137-153`), or pinning the camera at an empty-world preset. Architect's call.

### Q7 (interim hack removal)
Confirmed: the wasm32 short-circuit is at `crates/bevy_naadf/src/render/construction/mod.rs:944-957` (the handoff line range :920-940 is slightly off — the `#[cfg(target_arch = "wasm32")]` block actually starts at :944). The block's preamble starts at :926 ("WebGPU divergence" comment). It is the only wasm32-specific cfg-gated branch in `populate_cpu_mirror_from_gpu_producer`. Removing it after Q3's async path lands is a single-block delete.

Callers of `populate_cpu_mirror_from_gpu_producer`: registered as a render-app system in `crates/bevy_naadf/src/render/construction/plugin.rs` (the standard pattern — see `prepare_construction` registration); runs in the render schedule, one-shot per startup gated on `gpu_producer_has_run = true` (mod.rs:914). No other code path.

## Borderline calls

1. **`tracing::error!` counter for Q5.** Not reusable from existing code. The handoff explicitly asks for "zero `tracing::error!` calls fired during the run (the existing harness already has error-counter plumbing — look for `error_counter` or similar in `src/e2e/`)" — but no such counter exists. `PipelineScanResult` only counts pipeline errors. **Verdict was "not applicable" — flip to "extend" if** the architect chooses to install a custom `tracing` layer in `add_e2e_systems` that counts ERROR-level emissions (this is a small extension to `e2e/mod.rs`; the `tracing` crate's `Layer` API supports it cleanly).

2. **`bevy::tasks::AsyncComputeTaskPool` on wasm32.** `bevy::tasks::ComputeTaskPool` is already used (`world/data.rs:811-813`), but only on native — the threshold-gated parallel path skips it when `try_get().is_some()` returns `None`, which it does on `MinimalPlugins` tests. **Verdict "extend"** if `AsyncComputeTaskPool::get()` returns a valid pool on wasm32 with `webgpu` features. If it's a `spawn_local` shim under the hood, route #3 (JS WebWorker) becomes the only true off-main-thread option. The architect must verify against the actual `bevy_tasks` build for wasm32 — not knowable from the source alone without running a probe. This is the handoff's own Q1 unresolved question, restated.

3. **Reuse of `vox_gpu_oracle.rs`'s assertion *direction*.** The existing gate asserts **similarity** (SSIM ≥ 0.85). The new gate asserts **dissimilarity** (SSIM < ~0.95). Verdict "extend" — same metric, same helpers, different threshold orientation. The reused `compare_oracle_frames` body would need either (a) a new sibling function `compare_dissimilar_frames` with inverted Err/Ok logic, or (b) parameterise the comparison direction. (a) is cleaner — flip if the architect prefers (b) for DRY.

4. **`naadf_hash_coefficients_gpu_producer` + `naadf_segment_voxel_buffer_gpu_producer` missing COPY_SRC (production path, non-VOX).** Today neither is read back. Verdict "not applicable" because the new readback (Q3) only widens what's *already* read back today (no new buffers added to the readback set). **Flip to "extend"** if the architect decides the async readback should also read hash_coefficients (it shouldn't — that's the 65-entry constant table; nothing to gain) or segment_voxel_buffer (only for diagnostics, never a renderer dependency). Confirm with architect after Q3 route is chosen.

## Forbidden moves carried over

From the handoff:
- **No ranked-hypothesis lists.** The architect picks ONE route per question, not "A or B or C".
- **No verification claims based on `cargo run --bin bevy-naadf`.** Project rule (`CLAUDE.md`) — add an e2e gate, never boot the binary for verification.
- **No widening of test scope.** Deliver exactly one new native gate + one extended Playwright spec; do not rewrite existing gates.
- **No mocking of GPU work in tests.** Both gates run real WebGPU/Vulkan pipelines.
- **No skipping the explore phase.** This audit IS that explore phase.
- **No `--no-verify` on commits.** Project rule.
- **No headless-mode "fixes" for Playwright.** Memory `~/.claude/projects/-mnt-archive4-DEV-bevy-naadf/memory/playwright-e2e-must-be-headed.md` — always headed.
- **Do not move the 85 MB Oasis fixture into `dist/`.** Tests use `?vox=/test-fixtures/oasis_hard_cover.vox` via `e2e/serve.mjs`'s `/test-fixtures/` route.
- **Do not unify wasm `web_sys` dnd with native winit `FileDragAndDrop` dnd.** Different platforms, different APIs; the existing split is correct.
- **Do not retry sync `Device::poll(wait_indefinitely)` on WebGPU.** Reproduced multiple times; no-op for `mapAsync` awaiting.

Spotted in adjacent orchestrations (`docs/orchestrate/streaming-world/README.md`, `docs/orchestrate/web-chunks-storage-buffer/README.md`):
- **`web-chunks-storage-buffer`'s headed-mode pivot** (README.md "Headed-mode re-run" item) — confirms that `DeviceLost` in headless masks real WebGPU validation errors. The new spec must continue to fail headless reliably (the existing `test-wasm` recipe is `--headed` by default; keep it that way).
- **`web-chunks-storage-buffer`'s fixture lockstep rule** ("Fixture scope: all 5 sites lockstep") — if the new gate touches any fixture (no, it shouldn't), all 5 sites flip together. Inapplicable here but the rule is broader-relevant.
- **`streaming-world`'s "Block dedup: per-resident-chunk-local" + "Noise backend: voxel_noise"** — unrelated to async parse/readback. No carry-over forbidden moves.
