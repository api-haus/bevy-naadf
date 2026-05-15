# 00 — Reuse audit: taa-resize-blackness

## Goal recap

Fix a bug in `bevy-naadf` (`/mnt/archive4/DEV/bevy-naadf`, crate
`crates/bevy_naadf/`) where **shadow regions render pitch black after a window
resize** while TAA is enabled. The symptom is not a total-black frame but
specifically the GI-lit / TAA-accumulated shadow regions collapsing to black.
A prior partial fix (`18-taa-fidelity.md` fix #4, merged at `8995c88`) already
prevents the worst-case 1×1 buffer-collapse by retaining the last-known-good
viewport in `extract_camera`. The user's current report is therefore a residual
or secondary manifestation: on a **real** resize (new size IS valid, buffers
ARE legitimately rebuilt), `taa_samples` + `sample_counts` are zero-cleared,
so the 32-deep TAA ring and the 128-frame GI accumulation ring are both lost,
causing a ~32-frame / ~128-frame dark/shadowed-region recovery period during
which formerly-lit shadow regions read as black. The work is to be delivered
TDD-style — failing reproduction test first, fix second.

---

## Reuse candidates

| # | Existing artifact (path:lines) | What it does | Coverage of goal | Reuse / Extend / New |
|---|---|---|---|---|
| 1 | `extract_camera` in `crates/bevy_naadf/src/render/extract.rs:121-165` | Receives the camera viewport each `ExtractSchedule` frame; the **last-known-good retain** (fix #4) already guards against degenerate/None sizes by keeping `extracted.viewport_size` unchanged. The comment block (lines 137-159) is the canonical description of the resize-lifecycle contract. | Partially covers the resize-event handling chain. The specific residual bug lives downstream: `prepare_taa` + `prepare_gi` + `prepare_frame_gpu` still zero-clear on any `pixel_count` change, including legitimate resizes. | Extend — add a second guard or a "did viewport actually change?" signal so the fix #4 intent ("never shrink to bogus size; accept real size") is distinguishable from "started up with new size after bogus frame". |
| 2 | `prepare_taa` in `crates/bevy_naadf/src/render/taa.rs:286-464` | On a `pixel_count` change recreates `taa_samples` (pixel_count × ring_depth × 8 bytes), `taa_sample_accum`, `taa_dist_min_max`, and **zero-clears all three** via a `CommandEncoder::clear_buffer` submit (lines 379-387). `create_screen_buffers` (lines 476-506) is the allocation helper. This is the primary blackness source on real resizes: the 32-frame TAA ring is wiped, so `reproject_old_samples` finds nothing for ~32 frames. | Directly covers the buffer-lifecycle side. The recreation path (`Some(taa) => { create_screen_buffers(...) }` at lines 331-344) is the exact site that will be extended or guarded. | Extend — this is the primary target for the fix. Could be extended to (a) keep buffers at old size + trim/expand without zeroing, or (b) accept the zero-clear but ensure the camera-history matrices at the old projection are marked invalid so the reprojector doesn't silently reconstruct a mis-sized history. |
| 3 | `prepare_gi` in `crates/bevy_naadf/src/render/gi.rs:224-266` | On a `pixel_count` change recreates every `pixel_count`/`bucket_count`-sized GI buffer (lines 248-265) via `create_gi_buffers`, which zero-clears all of them including `sample_counts` — the 128-frame GI accumulation ring. Zero-clearing `sample_counts` on a resize means 128 frames before `refineBuckets` can pass its `< 12` gate (`sample_refine.wgsl:706-708`), during which `valid_samples_compressed` stays empty → `spatialResampling` finds no reservoirs → no indirect GI bounce in `final_color` → no contribution to `taa_sample_accum` → shadows read black. | Directly covers the GI-buffer lifecycle. Secondary blackness source on real resizes. | Extend — same class as candidate #2; the `create_gi_buffers` path is the site to guard or extend. |
| 4 | `e2e/driver.rs` (`E2eState`, `e2e_driver`) + `e2e/mod.rs` (`add_e2e_systems`, `E2E_WARMUP_FRAMES`, `E2E_MOTION_FRAMES`) | The bounded windowed e2e harness. Drives a deterministic WARMUP → MOTION → SETTLE → SHOOT → DRAIN → ASSERT state machine. Currently exercises static-camera convergence and camera-motion TAA reprojection, but the window size is fixed at `E2E_WIDTH = 256 × E2E_HEIGHT = 256` (lines 45-48 of `e2e/mod.rs`) and never resizes. The harness lacks a "trigger a resize event mid-run" phase. | Partial scaffold for a reproduction test — it drives the real windowed app with real GPU pipelines, reads back frames, and asserts luminance gates. A resize test would need a new E2e phase (between WARMUP and SHOOT) that programmatically resizes the window, then waits for the TAA ring to attempt recovery, then asserts the shadow region does NOT go black. | Extend — add a `RESIZE` phase to `E2ePhase` enum (`driver.rs:57-76`) that sends a `bevy::window::WindowResolution` event, lets the app run N frames, then reads back and gates on the solid/shadow region luminance. The existing WARMUP, MOTION, SETTLE, SHOOT/DRAIN/ASSERT infrastructure all reuses unchanged. |
| 5 | `e2e/gates.rs` `assert_batch_6` + `solid_block_rect` + `MIN_GI_BOUNCE_AFTER_MOTION` | The per-batch region gates. `assert_batch_6` (lines 537-600) asserts the GI-lit diffuse geometry's luminance ≥ `MIN_GI_BOUNCE_AFTER_MOTION = 150.0` after camera motion. The `solid_block_rect` region (lines 188-190) is exactly the region where a shadow-blackness regression would manifest — GI-lit shadow-band geometry at the fixed readback pose. | Directly reusable — the "shadows went black" failure mode maps precisely onto a `solid_block_rect` luminance collapse (measured ~242 healthy; collapse to ~4 is the failure). A resize reproduction test can call `assert_batch_6` (or inline its luminance check) after the resize phase. | Reuse — the gate function and rect are already exactly the right discriminator for the shadow-blackness symptom. No changes needed to the gate itself; the test just needs to call it after the resize phase. |
| 6 | `e2e/framebuffer.rs` (`Framebuffer`, `region_mean`, `luminance`, `check_luminance_alive`, `save_png`) | The CPU-side framebuffer wrapper used by all harness gates. Decodes the Bevy `Screenshot` image, normalises BGRA/RGBA, computes per-region means, saves PNG for visual inspection. | Fully covers the frame-capture + pixel-analysis need for a resize reproduction test. | Reuse — no changes needed. |
| 7 | `e2e/checks.rs` `scan_pipeline_errors_render_system` + `assert_nodes_dispatched` | The two ancillary harness checks: pipeline error scan (catches shader-compile failures post-resize) and node-dispatch check (catches a node going silent post-resize). | Partially covers the wider regression gate for a resize test — a resize that breaks a pipeline or silences a render-graph node would be caught here without new code. | Reuse — these systems are already wired by `add_e2e_systems` and run unconditionally. |

---

## Borderline calls

- **Candidate #1 (extract_camera) — "Extend" vs "already done".** The fix #4
  code is already merged and live. It prevents the bogus 1×1 case but explicitly
  accepts that a real resize zero-clears the TAA ring (the comment at lines
  578-580 of `18-taa-fidelity.md` fix section: "taa_samples / sample_counts are
  still legitimately lost on a REAL resize … the goal — eliminating the bogus
  1×1-collapse — is met"). Whether the residual "shadows go black for ~32 frames
  on a real resize" is the user's current complaint or a new/distinct one is the
  key uncertainty. If it is the SAME complaint (just the partial-fix residual),
  the fix direction is in `prepare_taa` (candidate #2) not `extract_camera`. If
  the user is seeing a NEW failure (e.g. the bogus-1×1 path still fires under
  some OS/Bevy-version condition), then `extract_camera` itself needs additional
  work. Verdict flips to "extend `extract_camera`" if the reproduction test shows
  a fully-black frame (not just dark-for-32-frames).

- **Candidate #2 (prepare_taa zero-clear) — "Extend" vs "New".** The zero-clear
  on a pixel_count change is the direct cause of the multi-frame dark recovery.
  Three fix strategies exist: (a) skip the zero-clear and trust the old-sized
  ring data is benign (may produce one frame of stale geometry at the new aspect
  ratio, then self-corrects), (b) introduce a "warm resize" that preserves or
  re-scales the ring, (c) preserve the zero-clear but force the reprojector to
  treat all history as invalid for one cycle explicitly rather than implicitly
  waiting for the ring to refill. Each is an extension of the existing
  `prepare_taa` function; none needs a new module. Flips to "New module" only if
  the chosen strategy requires a separate "TAA history preservation" resource with
  its own system ordering — unlikely.

- **Candidate #4 (e2e harness resize phase) — "Extend" vs "New test binary".** 
  The e2e harness cannot be a `#[test]` (winit main-thread constraint, documented
  in `e2e-render-test.md` §2.1). The resize reproduction test therefore needs to
  be expressed either as (a) a new `AppConfig` mode driven through `build_app` +
  a new `E2ePhase::Resize` state in the existing `driver.rs`, or (b) a separate
  `--resize` CLI mode in `e2e_render.rs`. Option (a) is an extension of the
  existing `e2e/driver.rs` state machine; option (b) is a small new mode in the
  existing `src/bin/e2e_render.rs`. Both are "extend existing" rather than new,
  but the borderline is how invasive the driver change is. Flips to "New" only if
  the Bevy API for programmatic window resize is incompatible with the bounded
  winit runner (unlikely — `Window::resolution` is writable at runtime).

---

## Bug surface map

### TAA history textures

The TAA screen-space buffers — `taa_samples`, `taa_sample_accum`,
`taa_dist_min_max` — are owned by `TaaGpu` resource
(`crates/bevy_naadf/src/render/taa.rs:234-266`). They are created and sized in
`prepare_taa` (lines 308-371); the resize trigger is
`taa.pixel_count != pixel_count` (lines 323-344). On any pixel_count change,
`create_screen_buffers` (lines 476-506) allocates fresh buffers and the three
are zero-cleared (lines 379-387). After a resize the `reproject_old_samples`
loop in `taa.wgsl:289` walks `for i in 1..sample_age` and finds all entries
zero — so `taa_sample_accum` stays near-zero for ~`ring_depth` (= 32) frames.

The camera-history ring (`TaaGpu.camera_history`, line 253) is fixed-size and
is NOT recreated on resize — it persists. However, the stored per-frame
`view_proj` / jitter entries were captured at the old projection, so after a
resize (which typically changes the aspect ratio) the reprojection maths
produce screen coordinates for the old aspect and the samples land outside the
new buffer bounds or on wrong pixels. This is a secondary source of
incorrect-looking geometry post-resize.

Key file:line references:
- Buffer ownership: `taa.rs:234-266` (`TaaGpu` struct)
- Recreation site: `taa.rs:308-371` (`prepare_taa` match arm)
- Allocation helper: `taa.rs:476-506` (`create_screen_buffers`)
- Zero-clear: `taa.rs:379-387` (encoder `clear_buffer` submits)
- WGSL ring walk: `assets/shaders/taa.wgsl:289` (`for i in 1..sample_age`)
- Ring depth const: `taa_common.wgsl:20` (`#{TAA_SAMPLE_RING_DEPTH}u`)

### Shadow textures

NAADF/bevy-naadf has no dedicated shadow-map texture — shadows are
ray-marched (sun visibility rays) during `spatial_resampling.wgsl`'s
`sample_neighbors` loop (lines 529-583, the multi-tap sun extension). The
"shadow" in the user's report refers to the GI-accumulated colour in
`taa_sample_accum` / `final_color` for regions that were receiving bounce
light through indirect GI: when `taa_samples` is zero-cleared, `CalcNewTaaSample`
(`naadf_calc_new_taa_sample.wgsl`) has no history to fold in, so the
accumulated colour for those regions drops to the single current-frame direct
estimate — which for shadow-band geometry (receiving only indirect GI, no
direct sun) is near-zero → pitch black.

The GI accumulation ring (`GiGpu.sample_counts`, `gi.rs:136`) is also
zero-cleared on a pixel_count change (`gi.rs:248-265` always calls
`create_gi_buffers` when `pixel_count` differs). `sample_counts` carries the
128-frame `globalIlumSampleCounts` ring; after a resize it takes ~128 frames
before `refineBuckets`'s `< 12` gate opens and `valid_samples_compressed`
is populated. Until then the spatial-resampling reservoir is empty → no
indirect GI bounce → shadow regions receive zero indirect light → black.

Key file:line references:
- Sun visibility (shadow rays): `spatial_resampling.wgsl:529-583`
- `sample_counts` lifecycle: `gi.rs:136` (field), `gi.rs:248-265` (recreation)
- `create_gi_buffers` (zero-clears all GI buffers): `gi.rs` (function, ~line 400+)
- `refineBuckets` `< 12` gate: `sample_refine.wgsl:706-708`
- GI buffer sizes: `gi.rs:47-60` (VALID_SAMPLE_STORAGE_COUNT etc.)
- `final_color` lifecycle: `prepare.rs:600-631` (same pixel_count trigger)

### Resize event handling

There is NO Bevy `WindowResized` event listener in the NAADF render code. The
resize signal propagates implicitly through `extract_camera`'s
`camera.physical_viewport_size()` poll (extract.rs:153-159). When the OS
delivers a new window size, Bevy's `camera_system` updates the camera's
viewport rect; the next `ExtractSchedule` run reads the new size and writes it
to `ExtractedCameraData.viewport_size` (provided it is non-degenerate). The
three prepare systems then compare their stored `pixel_count` against the new
value and rebuild if different.

The "degenerate guard" (fix #4) in `extract_camera` (lines 137-159) is the
only explicit resize-handling code. There is no centralized resize-recreate
helper; creation logic is scattered across three systems:
- `prepare_taa` — `taa.rs:286-464`
- `prepare_gi` — `gi.rs:224-500+`
- `prepare_frame_gpu` — `prepare.rs:443-800+`

All three independently compare `pixel_count` and rebuild. They share the same
upstream `extracted_camera.viewport_size` but have no coordinating resource.

### Stale-coord / stale-dimension risk sites

After a resize the following remain stale for one or more frames:

1. **`TaaGpu.camera_history` entries** — fixed-size buffer, not rebuilt.
   Entries at old-projection `view_proj` / `jitter` are still in the ring and
   will be read by `reproject_old_samples` for the next 32 frames
   (`taa.wgsl:289-~370`). The `screenPosDistanceSqr > 16.0` reject
   (`taa.wgsl:346`, `renderTaaSampleReverse.fx:139`) discards some of these
   (aspect-changed reprojected coords land far off-screen), but the reject
   uses the *new* screen dimensions so it may not discard all stale entries
   cleanly during the first ~128 frames.
   
2. **`GpuTaaParams.screen_width / screen_height`** — uploaded every frame in
   `prepare_taa` (lines 428-431) from the NEW `viewport` value, so this is
   current within the same frame. No stale risk here post-fix-#4.

3. **`GpuRenderParams.screen_width / screen_height`** — uploaded every frame
   in `prepare_frame_gpu` (`prepare.rs:523-524`). Current. No stale risk.

4. **`GpuGiParams.screen_width / screen_height / bucket_size_x / bucket_size_y`**
   — uploaded every frame in `prepare_gi` from the new `viewport` and
   `bucket_grid_of(viewport)`. Current. No stale risk.

5. **`FrameGpu.first_hit_data` bind-group references** — rebuilt when
   `needs_new_storage` is true (`prepare.rs:670-693`), i.e. when `pixel_count`
   changes. The `taa_reproject_bind_group` and `calc_new_taa_sample_bind_group`
   also reference `first_hit_data`, and both are rebuilt in the same
   `prepare_frame_gpu` call (`prepare.rs:694-787`). These are coherent within a
   frame — not a stale risk.

6. **`naadf_final_blit_node` fullscreen triangle** — reads from
   `taa_sample_accum` (bound in `blit_bind_group`). After a resize and the
   subsequent zero-clear, `taa_sample_accum` is all-zero for ~32 frames, making
   this the direct output path for the black-shadow symptom. Located in
   `render/graph.rs` (the `naadf_final_blit_node` function, around line 258-309
   per `18-taa-fidelity.md`).

---

## Test infrastructure inventory

### Headless rendering / frame capture

There is **no headless (GPU-free) render path** in this codebase. All rendering
requires the real Bevy `RenderPlugin` + wgpu backend + an on-screen window,
because the WGSL pipelines are runtime-compiled by naga-oil + the GPU driver.
The `e2e-render-test.md` §2.1 documents this constraint explicitly: winit
requires its event loop on the main thread; `cargo test` workers cannot host it;
therefore the e2e harness is a binary target, not a `#[test]`.

The existing harness in `crates/bevy_naadf/src/e2e/` drives a real windowed app:
- `e2e/mod.rs` — `add_e2e_systems`, `run_e2e_render`, `run_with_app`, constants
- `e2e/driver.rs` — `E2eState` / `E2ePhase` state machine + `e2e_driver` system
- `e2e/readback.rs` — `Screenshot::primary_window()` + `ScreenshotCaptured` observer
- `e2e/framebuffer.rs` — `Framebuffer` CPU-side analysis (BGRA/RGBA normalised)
- `e2e/gates.rs` — per-batch region gates + camera poses + `GateState`
- `e2e/checks.rs` — pipeline error scan + node-dispatch check

Frame capture is via Bevy's `Screenshot::primary_window()` entity, which reads
the on-screen window surface asynchronously. The `DRAIN` phase (`E2e_DRAIN_FRAMES
= 8`) polls for the `ScreenshotCaptured` event. The captured `Image` is decoded
by `Framebuffer::from_image`.

A resize reproduction test can reuse this entire pipeline; it needs only a new
`E2ePhase::Resize` state between `WARMUP` and `SHOOT`, plus an assertion that
the post-resize `solid_block_rect` region stays above `MIN_GI_BOUNCE_AFTER_MOTION`.

### Screenshot / image-diff helpers

There is **no image-diff helper** (no pixel-by-pixel baseline comparison, no
stored reference PNG). The harness uses luminance statistics on named screen
regions (`Framebuffer::region_mean`, `Framebuffer::luminance`) compared to
hard-coded thresholds. `Framebuffer::save_png` saves a PNG to
`target/e2e-screenshots/e2e_latest.png` for visual inspection.

`Framebuffer::stability_hash` (in `framebuffer.rs`) provides a `u64` hash of
the whole framebuffer, but all `hash_baseline` values are `None` (deliberately
— cross-GPU/cross-binary non-portability, per `gates.rs` lines 325-327). So
the only operative gates are the region-mean luminance checks.

These are fully sufficient for a shadow-blackness regression test: a
luminance collapse in `solid_block_rect` from ~242 to ~4 is unambiguous.

### Cargo test setup

`Cargo.toml` (`crates/bevy_naadf/Cargo.toml`) declares:
- `[lib]` at `src/lib.rs` — where all `#[cfg(test)]` modules live
- `[[bin]] name = "e2e_render"` at `src/bin/e2e_render.rs` — the windowed harness

There are **no `[[test]]` integration-test targets** and no `tests/` directory.
All 112 unit tests (`cargo test --lib`) are pure-CPU inline module tests in:
- `src/camera/position_split.rs:121-168` (camera split math)
- `src/voxel/grid.rs:341-420` (grid geometry)
- `src/aadf/cell.rs:205-340+` (cell encoding)
- `src/aadf/bounds.rs`, `src/render/construction/bounds_calc/tests.rs` etc.
  (AADF oracle + GPU construction bit-exact gates)
- `src/lib.rs` — TAA ring depth regression tests (added by `18-taa-fidelity.md`)
- `src/render/gpu_types.rs` — `size_of!` / `offset_of!` compile-time guards

A resize-triggered GPU behaviour test CANNOT be expressed as a `#[test]` (the
winit constraint). The only vehicle is the `e2e_render` binary. The failing
reproduction test must therefore be a new mode/phase within `cargo run --bin
e2e_render -- --resize-test` (or equivalent), not a `cargo test` entry.

---

## Recommended next step

The most feasible failing reproduction test is a new `E2ePhase::Resize` state
inserted into the existing `e2e/driver.rs` state machine, gated by a new
`AppArgs.resize_test: bool` flag and driven by writing a new
`WindowResolution` to Bevy's `Window` resource mid-run. The test shape would
be: run `E2E_WARMUP_FRAMES` to let GI converge → trigger a programmatic resize
(e.g. to `512×512`) → run ~`E2E_WARMUP_FRAMES` additional frames to let the
post-resize TAA ring attempt recovery → capture a frame and assert
`solid_block_rect` luminance stays ≥ `MIN_GI_BOUNCE_AFTER_MOTION`. A failing
test will show the solid region collapsing to ~4 (pitch-black shadows) in the
first few post-resize frames; the fix will hold it above the gate threshold by
either preserving the TAA ring across resize or recovering it faster. The test
reuses `add_e2e_systems`, `Framebuffer`, `assert_batch_6`'s gate logic,
`run_with_app`, and the existing `AppConfig::e2e` app wiring unchanged. The
primary fix target is `prepare_taa` (the zero-clear on pixel_count change at
`taa.rs:379-387`) and secondarily `prepare_gi` (`gi.rs:248-265`); the
fix-#4-era `extract_camera` degenerate-guard is not the root cause for the
residual symptom. The architect should also decide whether to preserve or
explicitly invalidate the 128-entry camera-history ring (which survives resize
at the old-projection matrices) — stale entries in that ring feed incorrect
reprojection coordinates for ~128 frames post-resize regardless of the
buffer-lifecycle fix.
