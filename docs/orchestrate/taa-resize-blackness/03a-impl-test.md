# 03a — Impl-A log: failing repro test

## Files changed
- `crates/bevy_naadf/src/lib.rs:228-243` — added `pub resize_test: bool` field to `AppArgs` (+ doc comment) and the matching `Default::default()` entry (`resize_test: false`).
- `crates/bevy_naadf/src/e2e/mod.rs:133-176` — added six new constants: `E2E_RESIZE_PRE_FRAMES = 180`, `E2E_RESIZE_POST_FRAMES = 120`, `E2E_RESIZE_WIDTH = 384`, `E2E_RESIZE_HEIGHT = 288`, `E2E_RESIZE_MIN_LUMA_RATIO = 0.5`, `E2E_RESIZE_PRE_PNG`, `E2E_RESIZE_POST_PNG`. The 60-fps wall-clock-approximation assumption is documented inline.
- `crates/bevy_naadf/src/e2e/mod.rs:164` — `init_resource::<driver::ResizeTestState>()` added to `add_e2e_systems`.
- `crates/bevy_naadf/src/e2e/driver.rs:37-58` — added `bevy::window::PrimaryWindow` import; pulled in `Rect`, `region_luminance_report`, and the new resize-test constants.
- `crates/bevy_naadf/src/e2e/driver.rs:78-114` — added eight new `E2ePhase` variants: `ResizePre`, `ResizeShootPre`, `ResizeDrainPre`, `ResizeDoIt`, `ResizePost`, `ResizeShootPost`, `ResizeDrainPost`, `ResizeAssert`.
- `crates/bevy_naadf/src/e2e/driver.rs:131-145` — added `ResizeTestState { pre: Option<Framebuffer>, post: Option<Framebuffer> }` resource.
- `crates/bevy_naadf/src/e2e/driver.rs:170-200` — `e2e_driver` signature gained two new params: `mut resize_test: ResMut<ResizeTestState>` and `mut window: Single<&mut Window, With<PrimaryWindow>>`. Added an up-front fast-path that jumps from `Warmup`/tick 0 straight to `ResizePre` when `app_args.resize_test == true`.
- `crates/bevy_naadf/src/e2e/driver.rs:325-470` — added all eight resize-phase arms. The resize is triggered by `window.resolution.set_physical_resolution(E2E_RESIZE_WIDTH, E2E_RESIZE_HEIGHT)` in `ResizeDoIt`. Each `*ShootPre/Post` arm spawns `Screenshot::primary_window()`, the matching `*DrainPre/Post` arm waits up to `E2E_DRAIN_FRAMES`, decodes the `Image` to a `Framebuffer`, and stashes it into `ResizeTestState`.
- `crates/bevy_naadf/src/e2e/driver.rs:475-580` — added `run_resize_test_assertions`. Computes `solid_block_rect`-shaped luma (`Rect::from_fractional(fb, 0.42, 0.52, 0.58, 0.66)`, identical to `gates::solid_block_rect`) on both screenshots. Compares post/pre ratio against `E2E_RESIZE_MIN_LUMA_RATIO = 0.5`. Also computes full-frame mean luma as a sanity check. Saves both PNGs (`resize_pre.png`, `resize_post.png`) and overwrites `e2e_latest.png` with the post-resize frame. On failure, the panic message includes both luma values, the ratio, the threshold, the PNG paths, and a pointer to `docs/orchestrate/taa-resize-blackness/`.
- `crates/bevy_naadf/src/bin/e2e_render.rs:71-110` — added `--resize-test` CLI flag parsing and a new branch that sets `app_args.resize_test = true` and dispatches through `bevy_naadf::run_e2e_render_with_args`.

## Test mechanism
The resize-test runs as a parallel state machine inside the existing e2e harness, **completely bypassing** the standard `Warmup→Motion→Settle→Shoot→Drain→Assert` flow. When `AppArgs.resize_test == true`, at the very first `Warmup` tick the driver re-routes to `ResizePre`.

Phase sequence:
1. **ResizePre** — 180 ticks (≈3 s at 60 fps; user's "waits 3 seconds" leg). The camera is pinned at the readback pose (`e2e_orbit_camera_transform(1.0)`) from frame 0, so the GI bounce converges where Batch-6's `solid_block_rect` discriminator is calibrated.
2. **ResizeShootPre** — one tick, spawns `Screenshot::primary_window()`.
3. **ResizeDrainPre** — up to 8 ticks waiting for the async capture; on arrival, decodes the `Image` to a `Framebuffer` and stashes it into `ResizeTestState.pre`.
4. **ResizeDoIt** — one tick, calls `window.resolution.set_physical_resolution(384, 288)` on the primary window.
5. **ResizePost** — 120 ticks (≈2 s at 60 fps; user's "waits 2 seconds" leg). Inside the user-observed recovery window (fractions-of-a-second to ~1–2 s).
6. **ResizeShootPost** + **ResizeDrainPost** — second screenshot + decode.
7. **ResizeAssert** — saves both PNGs, computes `region_luminance(solid_block_rect)` on each, prints luma + ratio + threshold + `region_luminance_report` for both, panics with `AppExit::error()` if `solid_post / solid_pre < 0.5`.

Luma comparison choice: the user said "compares luma values". I chose the `solid_block_rect` region as the discriminator (per the brief: "solid_block_rect is the targeted discriminator since the bug is shadow regions go pitch black"), with full-frame mean luma also printed as a sanity check. The `solid_block_rect` rect is fractional (`from_fractional(fb, 0.42, 0.52, 0.58, 0.66)`), so it transparently follows the post-resize resolution.

## Smoke run
- Command: `cargo run --bin e2e_render -- --resize-test`
- Exit code: **0** (test PASSED — but see "Notes / surprises" below; this is **not** the expected outcome on `main`).
- Pre-resize  `solid_block_rect` luma: **241.13**
- Post-resize `solid_block_rect` luma: **241.16**
- Ratio: **1.0001**
- Threshold: **0.50**
- Full-frame pre luma: **152.81**
- Full-frame post luma: **152.23**
- Panic / pass message: `e2e_render: resize-test PASS — pre/post luma ratio above threshold 0.5 after 180 pre-frames + window resize to 384x288 + 120 post-frames.`

## PNG outputs
- `target/e2e-screenshots/resize_pre.png` — saved (65 739 bytes, 256×256).
- `target/e2e-screenshots/resize_post.png` — saved (65 790 bytes, **256×256** — see "Notes / surprises").
- `target/e2e-screenshots/e2e_latest.png` — also overwritten with the post-resize frame (65 790 bytes, 256×256).

## Notes / surprises
**Critical finding — the resize did not actually take effect.** Both screenshots came out at **256×256**, not 384×288. Per the printed log:

```
e2e_render: resize-test triggered window resize to 384x288 (was 256x256)
e2e_render: resize-test pre  256x256 -> saved target/e2e-screenshots/resize_pre.png
e2e_render: resize-test post 256x256 -> saved target/e2e-screenshots/resize_post.png
```

The `window.resolution.set_physical_resolution(384, 288)` call ran (we logged it), but the actual window/surface dimensions did not change. Both the readback width and height stayed at 256×256, and the pre/post `solid_block_rect` luma values are essentially identical (241.13 vs 241.16, ratio 1.0001). This is exactly what one would expect if the resize was silently ignored: the rings were never zero-cleared, so there was nothing to drain.

**Per the brief's instruction: "If the smoke run shows the test passing against `main`, that's a signal something is off (likely the frame count is too long and the rings already recovered) — note that in your log and STOP. The user makes the next call."** The frame-count hypothesis can be ruled out from the data — the PNGs prove the resize did not propagate, not that the rings drained-and-recovered. Stopping per instruction.

Most likely cause: `WindowConfig::e2e()` sets `resizable: false` (`crates/bevy_naadf/src/lib.rs:271-281`). The architect (`02-design.md` §A.2) explicitly considered and rejected the hypothesis that this would block programmatic resize, citing that `resizable: false` only blocks *user*-driven resize-by-drag. Empirically that turned out to be wrong on this hardware/compositor (Wayland + NVIDIA + Bevy 0.19.0-rc.1), OR the Wayland compositor itself refused the size-change request, OR `bevy_winit`'s `changed_windows` short-circuits on `resizable: false` for the primary window.

**Recommended next-step orchestrator actions** (pick one before continuing to Impl-B):
1. Flip `resizable: true` on `WindowConfig::e2e()` and re-run Impl-A's smoke. (Smallest delta. May need a separate `WindowConfig::e2e_resize_test()` to keep the standard e2e bit-exact.)
2. Investigate `bevy_winit`'s response to programmatic resolution writes when `resizable: false` and document the actual gating logic.
3. Use a different resize trigger that ignores the `resizable` flag (e.g. `WindowMode` change, recreate the window — bigger change, last resort).

I deliberately did **not** flip `resizable: true` myself — that change is outside the Impl-A brief, and the brief explicitly says one smoke run maximum. The user/orchestrator makes the next call.

**Non-issue:** the `solid_block_rect` rect is fractional, so even if the post-resize frame had been 384×288 the rect would have followed transparently — no rect-recalibration debt.

**Test scaffolding compiles cleanly.** The new code path is fully gated by `AppArgs.resize_test == false` (default), so all existing batch gates and CI runs are unaffected.

**Choice of phase-routing.** I implemented the resize-test as a parallel state machine (immediate route from Warmup → ResizePre) rather than threading the existing Warmup→Motion→Settle. The architect's design (02-design.md §A.1) had it as a phase between Motion and Settle. The user's spec ("the test establishes a window, waits 3 seconds, screenshots, …") matches the parallel-machine reading more naturally — the 3-second pre-resize wait IS the warmup; there is no separate motion phase. The existing Warmup/Motion/Settle/Shoot/Assert flow runs unchanged when `--resize-test` is not passed. Architect's pose choice (the readback pose, where `solid_block_rect` is calibrated) is preserved.
