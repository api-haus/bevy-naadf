# 03c — Hypothesis pivot: the bug is NOT ring drain

## What was hypothesised (audit + architect)
Per `00-reuse-audit.md` and `02-design.md`, the original diagnosis was:
- TAA 32-frame ring zero-clear → 32-frame dark recovery
- GI 128-frame `sample_counts` ring zero-clear → 128-frame absent indirect bounce
- Combined effect: shadow regions go pitch black for ~32–128 frames post-resize

## What was tried (Impl-B, attempt 1)
Per `03b-impl-fix.md`: under user directive *"just reallocate all the buffers on resize, preserve nothing"*, Impl-B:
- Reverted match-arm structure in `prepare_taa` to single-arm allocation (collapsed three arms — `same pixel_count` / `Some(_) mismatch` / `None` — into `same pixel_count` / `_`).
- Force-reallocated `TaaGpu.camera_history` (the one ring previously preserved across resize) and zero-cleared it on the resize submit.
- Force-reallocated `TaaGpu.taa_params` (fixed-size uniform, previously cloned).
- Added a `reset_camera_history_on_resize` system on `WindowResized` to also zero the CPU-side `CameraHistory.{positions, view_proj, view_proj_inv, jitter}` rings.
- Reallocated `FrameGpu.camera` and `FrameGpu.render_params` uniforms unconditionally when `needs_new_storage`.
- Result: NO change in luma ratios. Pre-fix 0.5019/0.4769 → post-fix 0.5019/0.4771. Byte-identical to within measurement noise.

## What this tells us
The bug is NOT in the preserve-vs-reallocate axis. Forcing more reallocation didn't fix it; less reallocation (the architect's preserve-`sample_counts` plan) would not have either, because the metric being tested wasn't moving on the reallocation pivot at all. The actual failure mode is something else.

## New hypothesis (Impl-B agent's observation, preserved here)
The per-region pattern at the smoke output:
- sky: 167 → 160 → 154 (stable, only ~8% drop — atmosphere LUT is resolution-independent)
- emissive: 208 → 2.5 → 2.6 (≈99% drop)
- GI-lit solid: 204 → 16.7 → 15.3 (≈92% drop)
- Initial → resize A is ~4.3× more pixels (800×600 → 1920×1080); resize B is ~4.2× more.

This pattern is consistent with a **fixed per-frame ray / sample budget being spread thin over more pixels** at higher resolutions. The atmosphere LUT is unaffected because it doesn't depend on per-pixel ray casts. Emissive + GI solid collapse because they ARE per-pixel accumulated — and the per-pixel budget at 1920×1080 is roughly 1/4 the per-pixel budget at 800×600.

This also explains why **C# NAADF never had a visibly-acknowledged version of this bug** (see `00b-csharp-resize-research.md`): C# was used at fixed resolution, so the pixel count never changed dramatically; the per-pixel budget was always whatever it was tuned for.

## Implications
- **The audit's diagnosis was incomplete.** Ring drain may or may not contribute at all; the dominant cause appears to be resolution-dependent under-illumination, not temporal recovery.
- **The user's instinct ("just reallocate") was correct as a falsification** — it disproved the ring-drain hypothesis. The team now has empirical evidence that this isn't the axis to optimise.
- **"Recreate everything on resize" is NOT the way to fix this bug.** Documented per user directive.

## What to investigate next (for a future session)
Concrete next-step hypotheses to test:
1. **Find the per-frame ray / sample budget.** Likely candidates: `GpuRenderParams.ray_count`, `GpuGiParams.sample_budget`, `bucket_count`, `valid_samples_compressed` cap, or a constant like `RAY_BUDGET_PER_FRAME` in the WGSL shaders.
2. **Check whether that budget scales with `pixel_count`.** If not, that's the bug: a budget tuned for 800×600 is starving at 1920×1080.
3. **Possible fixes:**
   - Scale the budget linearly with `pixel_count` (faithful in spirit — gives each pixel the same number of rays it had at the lower resolution).
   - Adjust `MIN_GI_BOUNCE_AFTER_MOTION` threshold per resolution (cosmetic — masks the symptom without fixing it).
   - Investigate whether C# scales the budget (and if so, port that behavior).
4. **The repro test stays as the failing oracle** — once a real fix is identified, the same test (`cargo run --release --bin e2e_render -- --resize-test`) should report a pass.

## Files reverted
- `crates/bevy_naadf/src/render/taa.rs` — restored byte-identical to commit `85ea2fa` (pre-Impl-B). `use bevy::window::WindowResized` removed; `reset_camera_history_on_resize` system removed; `prepare_taa` match arms restored to the three-arm shape (`same pixel_count` / `Some(_) mismatch` / `None`); `camera_history` zero-clear removed from the resize submit.
- `crates/bevy_naadf/src/render/prepare.rs` — restored byte-identical to commit `85ea2fa`. `FrameGpu.camera` / `FrameGpu.render_params` reverted to the steady-state-clone path (`match &existing { Some => clone, None => create }`).
- `crates/bevy_naadf/src/lib.rs` — `reset_camera_history_on_resize` registration removed; `update_camera_history.after(reset_camera_history_on_resize)` ordering edge removed. Test scaffolding (`AppArgs.resize_test`, `WindowConfig::e2e_resize_test()`) preserved untouched.

Verification: `diff /tmp/<file>_pre.rs <repo>/<file>` reports identical for all three; `cargo check --bin e2e_render` builds clean.

## Files kept (test scaffolding)
- `crates/bevy_naadf/src/lib.rs` — `AppArgs.resize_test: bool` + `WindowConfig::e2e_resize_test()`
- `crates/bevy_naadf/src/e2e/{mod.rs,driver.rs,gates.rs}` — resize-test phases + camera pose
- `crates/bevy_naadf/src/bin/e2e_render.rs` — `--resize-test` CLI flag + pre-launch hyprctl windowrule

## Decision recorded
Per user message: *"yeah dawg youre right i conceed, lets pull those back and document that re-creating everything is not way to go"*. The reallocate-everything direction is closed. The faithful-port rule (`bevy-naadf-faithful-port-rule.md` in user memory) governs further fixes.
