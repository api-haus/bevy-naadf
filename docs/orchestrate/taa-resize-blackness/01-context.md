# 01 — Canonical context: taa-resize-blackness

> Every non-review agent MUST read this file in full before doing anything else. Review agents read `04-review.md` and ONLY `04-review.md` — they deliberately do NOT see this file (fresh-eyes generator–verifier separation).

---

## Goal — verbatim
User: *"lets fix the taa-resize blackness bug"* + clarification *"the shadows become pitch black"*.

Concretely: in `bevy-naadf` (this repo, crate `crates/bevy_naadf/`), after a window resize while TAA is enabled, **shadow regions render pitch black for a fraction of a second to ~1–2 seconds** before recovering. Confirmed by the user as **transient** behaviour — the rings drain, then refill. The user has confirmed scope: fix the **TAA + GI ring zero-clears** (not the deeper camera-history stale-projection issue, unless the architect proves it's load-bearing).

Delivery is **strict TDD**: a failing reproduction test goes in first via Impl-A. Hard gate confirms it fails. Then Impl-B applies the fix and verifies it passes.

---

## User decisions (Step-4 Q&A)
- **Bug symptom shape:** "Transient: shadows go black for a fraction of a second / a few frames after resize, then recover." (matches the auditor's diagnosis: 32-frame TAA ring drain + 128-frame GI `sample_counts` drain.)
- **Fix scope:** "TAA + GI rings (taa.rs + gi.rs zero-clears)." Camera-history stale-projection rebuild is **out of scope** for this pass unless the architect proves it's necessary to make the repro test pass.
- **TDD gating:** Strict — Impl-A writes the failing test, hard gate, Impl-B writes the fix.

---

## Bug pinpoints (from `00-reuse-audit.md`)

### Primary (TAA ring drain)
- `crates/bevy_naadf/src/render/taa.rs:286-464` — `prepare_taa`. On `taa.pixel_count != pixel_count` (lines 323-344) it recreates `taa_samples`, `taa_sample_accum`, `taa_dist_min_max` via `create_screen_buffers` (lines 476-506), then **zero-clears all three** at lines 379-387 via `CommandEncoder::clear_buffer` submits.
- After the zero-clear, `assets/shaders/taa.wgsl:289` (`reproject_old_samples` ring walk `for i in 1..sample_age`) walks 32 all-zero entries → `taa_sample_accum` near-zero for ~32 frames → indirect-lit shadow regions read black.
- Ring depth constant: `assets/shaders/taa_common.wgsl:20` (`#{TAA_SAMPLE_RING_DEPTH}u = 32`).

### Secondary (GI ring drain)
- `crates/bevy_naadf/src/render/gi.rs:224-266` — `prepare_gi`. On the same `pixel_count` mismatch (lines 248-265) it calls `create_gi_buffers`, which zero-clears every `pixel_count` / `bucket_count`-sized GI buffer including `sample_counts` (the 128-frame `globalIlumSampleCounts` ring).
- `assets/shaders/sample_refine.wgsl:706-708` — `refineBuckets`'s `< 12` gate stays closed until `sample_counts` re-accumulates ≥12 samples per bucket → `valid_samples_compressed` empty → `spatialResampling` finds no reservoirs → no indirect GI bounce → shadow regions black.

### Output path (where the black symptom surfaces)
- `crates/bevy_naadf/src/render/graph.rs` — `naadf_final_blit_node` (around line 258-309 per `18-taa-fidelity.md`). Reads from `taa_sample_accum` for the fullscreen blit; when that buffer is all-zero, the screen shows the black shadow regions.

### Already-correct (do NOT modify)
- `crates/bevy_naadf/src/render/extract.rs:121-165` — `extract_camera`'s last-known-good viewport retain (fix #4, commit `8995c88`). Prevents the 1×1 degenerate case. This is **NOT** the bug we're fixing; do not touch it.

### Out of scope (unless architect proves load-bearing)
- `crates/bevy_naadf/src/render/taa.rs:253` — `TaaGpu.camera_history` (128-entry ring). Survives resize at old-projection matrices. Causes stale reprojection coords for ~128 frames post-resize. **Scope-deferred** per user.

---

## Test infrastructure (from `00-reuse-audit.md`)

**No `#[test]` GPU path.** Winit needs the main thread; `cargo test` workers cannot host the event loop (documented in `docs/orchestrate/naadf-bevy-port/e2e-render-test.md` §2.1). All 112 unit tests are pure-CPU.

**The vehicle for the repro test is the existing `e2e_render` binary** at `crates/bevy_naadf/src/bin/e2e_render.rs`, which drives a real windowed `App` through a bounded state machine and reads back frames via `Screenshot::primary_window()`.

### Reusable scaffolding
- `crates/bevy_naadf/src/e2e/driver.rs` — `E2eState`, `E2ePhase` enum (`driver.rs:57-76`), `e2e_driver` system. **Add a new `E2ePhase::Resize` state.**
- `crates/bevy_naadf/src/e2e/mod.rs` — `add_e2e_systems`, `run_e2e_render`, `run_with_app`, `E2E_WIDTH = 256`, `E2E_HEIGHT = 256`, `E2E_WARMUP_FRAMES`, `E2E_MOTION_FRAMES`, `E2E_DRAIN_FRAMES`.
- `crates/bevy_naadf/src/e2e/readback.rs` — `Screenshot::primary_window()` + `ScreenshotCaptured` observer. Reuse unchanged.
- `crates/bevy_naadf/src/e2e/framebuffer.rs` — `Framebuffer`, `region_mean`, `luminance`, `save_png` to `target/e2e-screenshots/e2e_latest.png`. Reuse unchanged.
- `crates/bevy_naadf/src/e2e/gates.rs:537-600` — `assert_batch_6` (GI-lit diffuse-geometry luminance gate) + `solid_block_rect` at lines 188-190 + `MIN_GI_BOUNCE_AFTER_MOTION = 150.0`. **The exact discriminator** for the shadow-blackness symptom (healthy ~242, black collapses to ~4).
- `crates/bevy_naadf/src/e2e/checks.rs` — pipeline-error scan + node-dispatch check. Reuse unchanged.

### Test scaffold the auditor recommends
- New `AppArgs.resize_test: bool` flag (or equivalent) driving a new `E2ePhase::Resize` state between WARMUP and SHOOT.
- The resize is triggered by mutating Bevy's `Window::resolution` mid-run (e.g., to `512×512`).
- Architect decides exact frame counts and whether assertion fires on every post-resize frame or just on the SHOOT frame.

### What's NOT available (do not invent)
- No `image-diff` / baseline-PNG / golden-image helper. Cross-GPU non-portable; `Framebuffer::stability_hash` is deliberately not gated.
- No headless render path. Do not try to add one for this task.

---

## Required reading — verify with Read before designing
1. **Audit:** `docs/orchestrate/taa-resize-blackness/00-reuse-audit.md` (full)
2. **Bug surface — source:**
   - `crates/bevy_naadf/src/render/taa.rs:230-470` (TaaGpu struct + prepare_taa)
   - `crates/bevy_naadf/src/render/gi.rs:130-280` (GiGpu fields + prepare_gi up through buffer recreation)
   - `crates/bevy_naadf/src/render/extract.rs:121-165` (extract_camera; for context only — do NOT modify)
   - `assets/shaders/taa.wgsl:280-370` (reproject_old_samples ring walk + reject)
   - `assets/shaders/sample_refine.wgsl:700-720` (refineBuckets `< 12` gate)
3. **E2e harness — source:**
   - `crates/bevy_naadf/src/e2e/driver.rs` (full — small file)
   - `crates/bevy_naadf/src/e2e/mod.rs` (full)
   - `crates/bevy_naadf/src/e2e/gates.rs:1-200` for region rects + `:530-610` for assert_batch_6
   - `crates/bevy_naadf/src/bin/e2e_render.rs` (full — small)
4. **Prior orchestrate docs (port history):**
   - `docs/orchestrate/naadf-bevy-port/18-taa-fidelity.md` — the TAA implementation pass (esp. fix #4 around the line numbers cited in the audit)
   - `docs/orchestrate/naadf-bevy-port/20-impl-phase-d-shadow-A.md` — shadow rendering implementation
   - `docs/orchestrate/naadf-bevy-port/e2e-render-test.md` — the e2e-harness rationale, esp. §2.1 on the winit main-thread constraint
5. **Cargo workspace context:**
   - `crates/bevy_naadf/Cargo.toml` (lib + bins, no `[[test]]` integration targets)

---

## Forbidden moves
- **Do not** try to express the repro as `#[test]` / `cargo test`. The winit constraint is documented; respect it.
- **Do not** introduce a hash-baseline / stored-reference-PNG gate. Cross-GPU non-portable; the codebase deliberately uses only luminance-region gates.
- **Do not** modify `extract_camera`'s degenerate-guard (lines 137-159) — fix #4 is correct for the bogus-1×1 case and is not the bug we're fixing.
- **Do not** expand scope to the `TaaGpu.camera_history` stale-projection ring unless the repro test cannot pass without addressing it. If the architect believes it's load-bearing, escalate as an explicit `## Assumptions made` entry.
- **Do not** run the application repeatedly to "prove" the bug or "verify" a fix in-loop. **One smoke run maximum** per implementer dispatch (per the GPU-app verification-loop rule). The user does the visual confirmation.
- **Do not** rewrite the harness's screenshot path or framebuffer normalisation. Reuse `Framebuffer` and `Screenshot::primary_window()` as-is.
- **Do not** invent shader uniforms or bind-group entries for the fix — the existing layouts are correct; the fix is in the CPU-side buffer-lifecycle logic in `prepare_taa` / `prepare_gi`.
