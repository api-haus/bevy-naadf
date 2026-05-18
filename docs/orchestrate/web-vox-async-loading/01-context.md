# 01-context — web-vox-async-loading

Every non-review agent dispatched in this orchestration MUST read this file in full before doing anything else. Review agents read ONLY `05-review.md` and do NOT read this file (intentional — fresh-eyes pass).

## Goal (verbatim from handoff)

> Build a fully-featured async `.vox` loading pipeline on both web and native targets, and a closed-loop e2e suite (Playwright for web, the existing `e2e_render` Rust harness for native) that asserts both *no errors* and *pixels actually changed*.

The handoff numbers seven design questions (Q1–Q7) that the architect must answer with a chosen route + rationale + cost — **no ranked-hypothesis lists**. See "Architectural questions Q1–Q7" below for the full text.

## Worktree

- Path: `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/web-vox-streaming`
- Branch: `feat/web-vox-streaming`
- All paths in this document are repo-relative to that worktree.

## User constraints + decisions (from Step 4 Q&A on 2026-05-18)

These are **binding** for design and implementation. The architect MUST inline them into `03-architecture.md` as locked-in inputs and explain how each chosen route satisfies them.

### Decision 1 — Q4 scope (option a, **accepted**)
The reuse audit found that the three flagless W2 placeholder buffers (`segment_voxel_buffer_w2_placeholder`, `hash_map_w2_placeholder`, `hash_coefficients_w2_placeholder`) are **dead on the `.vox` production path** because the gate-widening at `crates/bevy_naadf/src/render/construction/mod.rs:1184-1186` routes `model_data = Some` runs through `naadf_*_gpu_producer` allocations instead, and none of the production-path source buffers that `populate_cpu_mirror_from_gpu_producer` reads from are missing `COPY_SRC` today.

- Q4 reduces to "confirm placeholders stay untouched on `.vox` runs".
- The implementer adds **no defensive `COPY_SRC` widening** to those three placeholders.
- The implementer adds a small test/assertion documenting that the placeholders aren't allocated on `.vox` runs (defends against future regressions to the gate logic at :1184-1186).
- See `00-reuse-audit.md` § "Q4 (other W2 placeholders' COPY_SRC)" for the full per-buffer table.

### Decision 2 — web `.vox` MUST build via the GPU pathway, identical to native (architectural directive, **non-negotiable**)
The user rejects any framing in which the web build can use a "CPU-only / non-production" path for `.vox` because "vox is loaded on CPU". On web the `.vox` must go through the **same GPU pipeline as native** — same `populate_cpu_mirror_from_gpu_producer` readback, same chunk/block/voxel buffers, same hash-keyed structure. This makes:

- **Q3 (async GPU readback)** load-bearing — the readback MUST work correctly on web through a real async path, not a wasm32 escape hatch.
- **Q7 (interim hack removal)** load-bearing, not optional polish. The interim wasm32 short-circuit at `crates/bevy_naadf/src/render/construction/mod.rs:944-957` (audit-confirmed line range; handoff cited :920-940 was off-by-preamble) **must be removed**. The async readback designed in Q3 replaces it.
- "Skip on web" is **not** an acceptable answer for any of Q1–Q7. Every choice must work on both targets.

### Decision 3 — Q1 web async parse route: `wasm-bindgen-rayon` (proven via `/mnt/archive4/DEV/bevy_pixel_world`)
Architect uses **`wasm-bindgen-rayon`** as the web async parse mechanism. The proven build configuration to copy is in `/mnt/archive4/DEV/bevy_pixel_world`. Specific files to read end-to-end:

| File | What to copy |
|---|---|
| `/mnt/archive4/DEV/bevy_pixel_world/.cargo/config.toml` | the entire `[target.wasm32-unknown-unknown]` block — `rustflags` with `+simd128,+atomics,+bulk-memory,+mutable-globals` and the `link-arg`s for `--shared-memory`, `--max-memory=1073741824`, `--import-memory`, `--export=__wasm_init_tls`, `--export=__tls_size`. The file's own comment confirms "Required for SharedArrayBuffer (wasm-bindgen-rayon)". |
| `/mnt/archive4/DEV/bevy_pixel_world/rust-toolchain.toml` | nightly toolchain pin (`nightly-2025-11-15` at time of writing) with `rust-src` for build-std. |
| `/mnt/archive4/DEV/bevy_pixel_world/crates/game/Cargo.toml` | rayon + wasm-bindgen + wasm-bindgen-futures wiring. Confirm whether `wasm-bindgen-rayon` is direct or transitive. |
| `/mnt/archive4/DEV/bevy_pixel_world/crates/game/Trunk.toml` | COOP/COEP serve headers (already mirrored in bevy-naadf — confirm parity). |
| `/mnt/archive4/DEV/bevy_pixel_world/crates/game/index.html` | the loading-overlay DOM is nearly identical to bevy-naadf's; confirm `#loading`, `#progress-fill.indeterminate`, `#progress-text`, `window.hideLoading`, `window.updateLoadingProgress` parity. |

The architect's job in `03-architecture.md` is to write the *exact* delta against bevy-naadf's current `crates/bevy_naadf/Cargo.toml` + `crates/bevy_naadf/_headers` + `crates/bevy_naadf/index.html` + `crates/bevy_naadf/init.js.template` + (new) `.cargo/config.toml` / `rust-toolchain.toml` (or workspace equivalents). If `wasm-bindgen-rayon` is **not** in bevy_pixel_world's Cargo.toml directly, the architect MUST verify what mechanism provides the worker pool there and document any deviation.

### Decision 4 — Q6 Playwright SSIM: shell out to Rust binary wrapping `image-compare`
The Playwright spec compares baseline-skybox.png vs vox-loaded.png by spawning a tiny Rust binary that wraps `image-compare`'s `Algorithm::MSSIMSimple`. Either (a) add a new bin like `e2e_image_compare`, or (b) expose a `--ssim-compare <a.png> <b.png>` flag on an existing bin (e.g. `e2e_render` itself). Architect picks (a) or (b) and justifies. The goal: **zero metric drift** between the native gate and the Playwright gate — same SSIM impl on both sides.

## Reuse audit — top candidates (read `00-reuse-audit.md` for the full table)

From `00-reuse-audit.md` § Candidates table (13 entries). The architect MUST read the full audit before designing.

1. **`crates/bevy_naadf/src/e2e/vox_gpu_oracle.rs` (entire file)** — the SSIM-compare template the new native gate extends. Helpers `load_png_as_framebuffer`, `framebuffer_to_rgb_image`, `save_oracle_screenshot`, `compare_oracle_frames` (`vox_gpu_oracle.rs:471-582`), the two-phase subprocess orchestration (`run_vox_gpu_oracle_compare` at `:346-463`), the camera pinning (`pin_vox_gpu_oracle_camera` at `:642-659`), the sanity guards (`count_pixels_with_luminance_above`/`_below`) — all reusable verbatim. Only the assertion direction inverts (SSIM **<** threshold for dissimilarity vs `≥` for similarity).
2. **`parse_vox_bytes` (`crates/bevy_naadf/src/voxel/vox_import.rs:154-225`) + `install_vox_bytes_in_fixed_world` (`crates/bevy_naadf/src/voxel/grid.rs:325-450`)** — the parse + Bevy-install split is already done; both already take `&[u8]`; both already feed native + web entry points. The async work bolts on top of this seam, not into it.
3. **`crates/bevy_naadf/src/voxel/web_vox.rs:34-72,184-292,295-338`** — single-slot inbox (`PENDING_VOX_BYTES`/`QUEUED_FOR_INSTALL`) + drag-drop shim + two-stage deferred parse + DOM overlay helpers. The seam to extend is the **stage-2 sync install** body of `apply_pending_vox`. The inbox, the overlay code, and stage-1 stay untouched.

Per-question coverage map (Q1–Q7) is in `00-reuse-audit.md` § "Per-question coverage map".

## Required reading (architect + implementer; read in order)

**Source handoff (read end-to-end):**

- `/tmp/web-vox-async-loading-handoff.md` — the full handoff document. Reasons-why and forbidden-moves the user has already named.

**Current sync-load pipeline (web entry points + shared install):**

- `crates/bevy_naadf/src/voxel/web_vox.rs` — entire file. Single-slot inbox, `wasm_bindgen_futures::spawn_local` HTTP fetch, drag-drop closure on `document.body`, two-stage deferred-parse `apply_pending_vox`, loading-overlay DOM helpers.
- `crates/bevy_naadf/src/voxel/grid.rs:297-394` — `install_vox_in_fixed_world` (fs wrapper) + `install_vox_bytes_in_fixed_world` (bytes core; native + web).
- `crates/bevy_naadf/src/voxel/grid.rs:395-465` — `native_vox_drop_listener` (winit `FileDragAndDrop` events) + `log_native_dnd_registered`.
- `crates/bevy_naadf/src/voxel/vox_import.rs:154-200` — `parse_vox_bytes` / `parse_vox_bytes_tiled` / `parse_dot_vox_data` (the actual `dot_vox` parse + `ImportedVox` build — multi-second CPU bottleneck).
- `crates/bevy_naadf/src/voxel/vox_import.rs:286-323` — `build_world_from_vox`.

**Synchronous-readback panic site (the wasm divergence Q3 must replace):**

- `crates/bevy_naadf/src/render/construction/mod.rs:897-1045` — `populate_cpu_mirror_from_gpu_producer`. Inner `readback_u32` closure at `:932-956` uses the sync `slice.map_async(...)` + `render_device.poll(PollType::wait_indefinitely()).unwrap()` + `slice.get_mapped_range()` pattern. `Device::poll(wait_indefinitely)` is **a no-op on WebGPU** — confirmed multiple times — so `get_mapped_range` runs before the buffer is mapped and panics with `OperationError: Failed to execute 'getMappedRange' on 'GPUBuffer'`.
- `crates/bevy_naadf/src/render/construction/mod.rs:944-957` — the **interim wasm32 short-circuit** (audit-confirmed line range; handoff cited :920-940 was off-by-preamble). Q7 mandates removing this entire block; Decision 2 above promotes Q7 from "polish" to "load-bearing".
- `crates/bevy_naadf/src/render/construction/mod.rs:1184-1186` — the gate-widening that routes `model_data = Some` runs through `naadf_*_gpu_producer` (the reason Q4 is moot — see Decision 1).
- `crates/bevy_naadf/src/render/construction/mod.rs:1882-1900` — the four W2 placeholder buffers. Confirm via audit that three of them are dead on the `.vox` path.
- `crates/bevy_naadf/src/world/buffer.rs:33-39` — `GROWABLE_BUFFER_USAGES = STORAGE | COPY_SRC | COPY_DST` (already correct).

**Existing native e2e harness (Q5 template):**

- `crates/bevy_naadf/src/bin/e2e_render.rs` — entry point. Named gates: `--baseline`, `--validate-gpu-construction`, `--edit-mode`, `--entities`, `--vox-e2e`, `--oasis-edit-visual`, `--runtime-edit-mode`, `--vox-gpu-oracle`. Each boots a windowed render, captures framebuffer PNGs to `target/e2e-screenshots/`, runs assertions.
- `crates/bevy_naadf/src/e2e/mod.rs` — `add_e2e_systems` (line 200-279), `AppConfig::e2e`. Native DnD currently gated **off** in e2e via `lib.rs:721-724`.
- `crates/bevy_naadf/src/e2e/vox_gpu_oracle.rs` — entire file. **The SSIM template** — read end-to-end before extending.
- `crates/bevy_naadf/src/e2e/framebuffer.rs:207` (`from_raw_rgba`), `save_png`, `mean_pixel_delta`, `region_mean`, `Framebuffer::luminance` — framebuffer wrapper + PNG IO.
- `crates/bevy_naadf/src/e2e/driver.rs:200-247` — `E2ePhase` extensions, `E2eOutcome`, screenshot drain phases.
- `crates/bevy_naadf/src/e2e/checks.rs` + `crates/bevy_naadf/src/e2e/mod.rs:269-278` — `PipelineScanResult` (pipeline errors, not `tracing::error!` ERROR-level emissions; the latter has **no existing counter** and is a borderline call — see audit).

**Existing Playwright infrastructure (Q6 template):**

- `e2e/playwright.config.ts` — `channel: "chrome"` (system Chrome required).
- `e2e/serve.mjs` — static server with COOP/COEP headers + `/test-fixtures/` route serving `crates/bevy_naadf/assets/test/`.
- `e2e/tests/helpers/console-collector.ts` — `ConsoleCollector`. Bevy ERROR-level logs surface as `console.log` with `%cERROR%c` CSS markers — the collector parses the marker. `IGNORED_PATTERNS` checks both message text and `msg.location().url`.
- `e2e/tests/wasm-smoke.spec.ts` — existing boot-and-render smoke (panic-free + canvas-visible).
- `e2e/tests/vox-loading.spec.ts` — the currently-red spec built in the previous session. Uses `?vox=/test-fixtures/oasis_hard_cover.vox` query override + same-origin local fetch. Always headed.
- `justfile:106-145` — `install-e2e`, `test-wasm` (headed), `test-wasm-headless` (diagnostic-only), `test-wasm-full` recipes.

**Loading-overlay DOM (already-wired, reuse don't reinvent):**

- `crates/bevy_naadf/index.html:1-138` — `#loading`, `#progress-fill` (with `.indeterminate`), `#progress-text`, `window.hideLoading`, `window.updateLoadingProgress(loaded, total)`.
- `crates/bevy_naadf/init.js.template:17-22` — streaming-fetch progress pattern (`ReadableStream` + `getReader().read()` loop).
- `crates/bevy_naadf/_headers:8-9` — `Cross-Origin-Opener-Policy: same-origin` + `Cross-Origin-Embedder-Policy: require-corp`. SharedArrayBuffer is already enabled in production.

**Reference codebase for `wasm-bindgen-rayon` integration:**

- `/mnt/archive4/DEV/bevy_pixel_world/.cargo/config.toml` — full WASM rustflags + link-args block.
- `/mnt/archive4/DEV/bevy_pixel_world/rust-toolchain.toml` — nightly pin + `rust-src` for build-std.
- `/mnt/archive4/DEV/bevy_pixel_world/crates/game/Cargo.toml` — deps.
- `/mnt/archive4/DEV/bevy_pixel_world/crates/game/Trunk.toml` — COOP/COEP serve headers.
- `/mnt/archive4/DEV/bevy_pixel_world/crates/game/index.html` — loading overlay parity check.

**Existing AssetLoader registrations (Q2 inspiration):**

- `crates/bevy-instamat/src/baked_material.rs:215-223` — `register_asset_loader(MaterialRonLoader)`.
- `crates/bevy_naadf/src/texture_array/{loader,mod}.rs` — `register_asset_loader(TextureArrayLoader)`.

## Existing changes in the worktree (do NOT redo)

```
 M .github/workflows/deploy-cloudflare.yml       # R2 upload step for the .vox
 M Cargo.lock
 M crates/bevy_naadf/Cargo.toml                  # wasm-bindgen + web-sys deps
 M crates/bevy_naadf/src/lib.rs                  # wires web_vox + native DnD
 M crates/bevy_naadf/src/render/construction/mod.rs  # COPY_SRC + interim wasm skip
 M crates/bevy_naadf/src/voxel/grid.rs           # install_vox_bytes_in_fixed_world split
 M crates/bevy_naadf/src/voxel/mod.rs            # web_vox module gate
 M e2e/serve.mjs                                 # /test-fixtures/ route
 M e2e/tests/helpers/console-collector.ts        # favicon/url-loc filter
 M justfile                                      # headed-only e2e recipes
?? crates/bevy_naadf/src/voxel/web_vox.rs        # async fetch + dnd shim + overlay
?? e2e/tests/vox-loading.spec.ts                 # web e2e spec (currently red)
```

## Architectural questions Q1–Q7 (from handoff — architect must answer each)

The architect writes one paragraph per question in `03-architecture.md` with a chosen route + rationale + cost. **No ranked-hypothesis lists.** The user's Q&A decisions above pre-empt some of these — re-state the decision and explain the *exact integration*, do not re-deliberate.

1. **Async parse on web — choice + cost.** Decision 3 above locks `wasm-bindgen-rayon`. Architect documents: exact crate version, exact `.cargo/config.toml` changes vs bevy-naadf's current state, nightly toolchain pin proposed, whether `init_thread_pool()` is called from a Trunk-side JS shim or from a wasm-bindgen entry export, how `parse_vox_bytes` is dispatched onto the pool (rayon `spawn` or `ThreadPool::spawn`), how the `Send`-bound completion is plumbed back to the main thread (oneshot channel? Bevy `Task`?), how the loading overlay's "Parsing model…" + indeterminate progress bar is tied to the rayon task lifecycle.
2. **Async parse on native — strategy.** Architect picks Bevy `AssetLoader<ImportedVox>` vs `AsyncComputeTaskPool::get().spawn(...)` with poll-in-`Update`. Apply same approach to both `Startup` boot path and `native_vox_drop_listener` (`grid.rs:471-529`).
3. **Async GPU readback for `populate_cpu_mirror_from_gpu_producer`.** Pick one approach that works on BOTH targets (Decision 2). Options listed in handoff are: (a) make function async, drive from `AsyncComputeTaskPool`, await mapping via oneshot; (b) convert to cross-frame state machine (issue copy + map_async frame N, check `MapState::Mapped` frame N+1, populate frame N+2); (c) skip on web — **forbidden by Decision 2**. Pick (a) or (b) and justify.
4. **Other W2 placeholder buffers.** Decision 1 locks: confirm placeholders untouched, no `COPY_SRC` widening. Architect documents the assertion the implementer will add.
5. **Native e2e gate design.** New gate (`--vox-web-parity` is the working name) extending `vox_gpu_oracle.rs` template. Architect specifies: gate name + sub-modes (`-skybox`/`-loaded` two-phase subprocess), whether skybox-empty mode is a new `GridPreset::Empty` variant or a new `AppArgs` boolean, the SSIM threshold + sanity guard direction inversion, the `tracing::error!` counter design (no existing counter — see audit borderline call 1; architect picks: install a custom `tracing` layer in `add_e2e_systems` OR widen `PipelineScanResult` OR scope to "pipeline errors only").
6. **Playwright SSIM gate.** Decision 4 locks: shell out to a Rust binary wrapping `image-compare`. Architect specifies: new bin (`e2e_image_compare`) vs flag on `e2e_render`; CLI shape; exit-code semantics; how the Playwright spec captures the skybox baseline (`?skybox=1` query string vs reusing the empty-preset mechanism from Q5).
7. **Cleanup.** Remove the interim wasm32 short-circuit at `construction/mod.rs:944-957`. Decision 2 makes this load-bearing.

## Verification hard rules

These bind the implementer in `04-refactoring.md`. The reviewer in `05-review.md` enforces them.

- **`cargo run --bin bevy-naadf` is forbidden as a verification step.** Project rule (`CLAUDE.md`). The deterministic gates are the verification surface. Any runtime behaviour needing programmatic proof gets **a new e2e gate**, not a binary smoke.
- **e2e gates need wall-clock budgets + diagnostic bail.** Per `~/.claude/projects/-mnt-archive4-DEV-bevy-naadf/memory/feedback-e2e-gates-must-fail-fast.md`: every `while !condition { sleep / yield }` loop in the new gate gets `Instant::now()` + `Duration::from_secs(N)` with `N` ≤ ~60s for the entire gate. On budget exhaustion the gate **prints a diagnostic** explaining what state was still waiting (which slots, which buffers, which frame counter). When the implementer runs `cargo run --bin e2e_render -- ...` for verification, wrap in `timeout 120s` for belt-and-braces.
- **Playwright always headed.** Per `~/.claude/projects/-mnt-archive4-DEV-bevy-naadf/memory/playwright-e2e-must-be-headed.md`: headless Chromium WebGPU dies with `DeviceLost` mid-render and hides real failures. The `test-wasm` recipe is headed-only by default. Do not "fix" by working around `DeviceLost`.
- **No mocking of GPU work in tests.** Both gates run real WebGPU/Vulkan pipelines. If a test passes only against a mock, it proves nothing.
- **Faithful-port rule.** Per `~/.claude/projects/-mnt-archive4-DEV-bevy-naadf/memory/bevy-naadf-faithful-port-rule.md`: no Bevy-only behaviour divergences from C# NAADF without explicit user approval. Async loading is a **web platform** divergence (C# port doesn't have a web target), so this rule is **permissive here**, but any behavioural change visible in native rendering needs justification in `03-architecture.md`.

## Forbidden moves (from handoff + carried-over)

- **No ranked-hypothesis lists.** "Maybe it's A, or B, or C" is the failure mode the handoff exists to prevent. Read required files end-to-end and decide.
- **No verification claims based on running `bevy-naadf`** (project rule above).
- **No widening of test scope.** Deliverable is ONE new native gate + extended `vox-loading.spec.ts`. Do not also rewrite the existing gates.
- **No mocking of GPU work in tests** (rule above).
- **No skipping ahead without the explore phase** — this audit IS that explore phase.
- **No `--no-verify` on commits** (project rule).
- **No headless-mode "fixes"** for Playwright (rule above).
- **No regression to the live R2 URL for tests.** Tests use `?vox=/test-fixtures/oasis_hard_cover.vox` via `e2e/serve.mjs`. R2 may not have the right key on every branch.
- **Do NOT unify wasm `web_sys` dnd with native winit `FileDragAndDrop` dnd.** Different platforms, different APIs; the existing split is correct.
- **Do NOT move the 85 MB Oasis fixture into `dist/`.** Production fetches from R2; tests fetch same-origin via `/test-fixtures/`.
- **Do NOT retry sync `Device::poll(wait_indefinitely)` on WebGPU.** No-op for `mapAsync` awaiting — reproduced multiple times.
- **No "skip on web" answer** to any Q1–Q7 (Decision 2).

## Image / conversation references

This handoff contains **no inline images**. All references in this file are paths or prose. If any sub-agent feels the need to reference "the screenshot above" or "image N", they have lost context — re-read this document.

## Memory entries that bind this work

- `~/.claude/projects/-mnt-archive4-DEV-bevy-naadf/memory/playwright-e2e-must-be-headed.md`
- `~/.claude/projects/-mnt-archive4-DEV-bevy-naadf/memory/bevy-naadf-faithful-port-rule.md`
- `~/.claude/projects/-mnt-archive4-DEV-bevy-naadf/memory/subagent-gpu-app-verification-loop.md`
- `~/.claude/projects/-mnt-archive4-DEV-bevy-naadf/memory/feedback-e2e-gates-must-fail-fast.md`

The architect/implementer should treat the contents of these as binding rules even if not re-stated in their brief. Inline summaries above already capture the load-bearing parts.
