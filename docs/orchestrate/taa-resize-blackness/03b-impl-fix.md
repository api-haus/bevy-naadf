# 03b — Impl-B log: reallocate all buffers on resize, preserve nothing

## User directive (verbatim)
> "tests fail. good. now fix the TAA blackness resize. just reallocate all the buffers on resize, preserve nothing."

## Files changed
- `crates/bevy_naadf/src/render/taa.rs:15-22` — added `use bevy::window::WindowResized;` for the new resize-detection system.
- `crates/bevy_naadf/src/render/taa.rs:227-273` — added new main-world `Update` system `reset_camera_history_on_resize`: zero-resets the 128-entry `CameraHistory` rings (`positions`, `view_proj`, `view_proj_inv`, `jitter`) whenever a `WindowResized` event fires. `frame_count` / `taa_index` / `current_jitter` deliberately NOT reset (monotonic / derived).
- `crates/bevy_naadf/src/render/taa.rs:315-371` (the `prepare_taa` match) — collapsed the previous three-arm match (`same pixel_count` / `Some(_) mismatch` / `None`) into `same pixel_count` / `_`. The `_` arm now ALLOCATES fresh `taa_samples`, `taa_sample_accum`, `taa_dist_min_max`, **`camera_history`**, **`taa_params`** — no more cloning of `camera_history` / `taa_params` on resize.
- `crates/bevy_naadf/src/render/taa.rs:373-394` — added an explicit `clear_buffer(&camera_history, 0, None)` to the resize zero-clear submit. The other three buffers (`taa_samples`, `taa_sample_accum`, `taa_dist_min_max`) are unchanged.
- `crates/bevy_naadf/src/render/prepare.rs:634-685` — restructured the uniform-buffer creation in `prepare_frame_gpu`. Was: `match &existing { Some => clone, None => create }`. Now: `if needs_new_storage { create fresh } else { clone (existing must be Some) }`. So `FrameGpu.camera` and `FrameGpu.render_params` are recreated whenever the storage buffers are (i.e. whenever `pixel_count` changes OR first build).
- `crates/bevy_naadf/src/lib.rs:561-582` — registered the new `reset_camera_history_on_resize` system in the `Update` schedule, with `update_camera_history` ordered `.after(reset_camera_history_on_resize)` so a resize-frame produces: zero-reset → write this frame → advance frame_count.

## Inventory of preserved-across-resize resources (now force-reallocated)
Full table in `03b-realloc-inventory.md`. Summary:

### Forced changes (were previously preserved):
1. `TaaGpu.camera_history` GPU buffer — was cloned across resize. Now recreated + zero-cleared.
2. `TaaGpu.taa_params` GPU buffer — was cloned across resize. Now recreated (no clear needed, overwritten per frame).
3. `FrameGpu.camera` GPU uniform — was cloned. Now recreated on resize.
4. `FrameGpu.render_params` GPU uniform — was cloned. Now recreated on resize.
5. Main-world `CameraHistory.{positions, view_proj, view_proj_inv, jitter}[128]` — survived resize at old-projection matrices. Now zero-reset via the new `WindowResized` event listener.

### Already reallocated + zero-cleared on resize (no change):
- All `TaaGpu` screen-space buffers (`taa_samples`, `taa_sample_accum`, `taa_dist_min_max`).
- All `GiGpu` buffers — `create_gi_buffers` already does a wholesale recreate including `sample_counts` and the indirect-dispatch buffers.
- All `FrameGpu` screen-space buffers (`first_hit_data`, `first_hit_absorption`, `final_color`).
- All bind groups (rebuilt on `needs_new_storage`).

## Code changes — per-file diff sketch

### crates/bevy_naadf/src/render/taa.rs

Before (resize arm of `prepare_taa`):
```rust
Some(taa) => {
    let (taa_samples, taa_sample_accum, taa_dist_min_max) =
        create_screen_buffers(&render_device, pixel_count, ring_depth);
    (taa_samples, taa_sample_accum, taa_dist_min_max,
     taa.camera_history.clone(),     // <-- PRESERVE
     taa.taa_params.clone(),         // <-- PRESERVE
     true)
}
None => { /* create everything */ }
```

After:
```rust
_ => {
    // First build OR resize — create EVERYTHING fresh.
    // reallocate-all-on-resize: per user directive 2026-05-15 — preserve nothing.
    let (taa_samples, taa_sample_accum, taa_dist_min_max) =
        create_screen_buffers(&render_device, pixel_count, ring_depth);
    let camera_history = render_device.create_buffer(...);   // <-- RECREATE
    let taa_params = render_device.create_buffer(...);       // <-- RECREATE
    (taa_samples, ..., camera_history, taa_params, true)
}
```

And the zero-clear submit gained one line:
```rust
encoder.clear_buffer(&taa_samples, 0, None);
encoder.clear_buffer(&taa_sample_accum, 0, None);
encoder.clear_buffer(&taa_dist_min_max, 0, None);
encoder.clear_buffer(&camera_history, 0, None);  // <-- NEW
```

New system added:
```rust
pub fn reset_camera_history_on_resize(
    mut events: MessageReader<WindowResized>,
    mut history: ResMut<CameraHistory>,
) {
    if events.is_empty() { return; }
    events.clear();
    history.positions     = [PositionSplit::default(); CAMERA_HISTORY_DEPTH];
    history.view_proj     = [Mat4::IDENTITY; CAMERA_HISTORY_DEPTH];
    history.view_proj_inv = [Mat4::IDENTITY; CAMERA_HISTORY_DEPTH];
    history.jitter        = [Vec2::ZERO; CAMERA_HISTORY_DEPTH];
}
```

### crates/bevy_naadf/src/render/gi.rs
**No code changes.** `prepare_gi`'s existing `_ => create_gi_buffers(...)` path already reallocates + zero-clears every buffer including the fixed-size `sample_counts` and the indirect-dispatch buffers. Inventory confirmed no preservation here today.

### crates/bevy_naadf/src/render/prepare.rs
Before:
```rust
let (camera_buf, render_params_buf) = match &existing {
    Some(frame) => (frame.camera.clone(), frame.render_params.clone()),
    None => { /* create both */ }
};
```

After:
```rust
let (camera_buf, render_params_buf) = if needs_new_storage {
    // reallocate-all-on-resize: per user directive 2026-05-15 — preserve nothing.
    let camera_buf = render_device.create_buffer(...);
    let render_params_buf = render_device.create_buffer(...);
    (camera_buf, render_params_buf)
} else if let Some(frame) = &existing {
    (frame.camera.clone(), frame.render_params.clone())
} else {
    unreachable!(...)
};
```

### crates/bevy_naadf/src/lib.rs
Before:
```rust
app.add_systems(
    Update,
    render::taa::update_camera_history.after(camera::sync_position_split),
);
```

After:
```rust
app.add_systems(
    Update,
    render::taa::update_camera_history
        .after(camera::sync_position_split)
        .after(render::taa::reset_camera_history_on_resize),
);
app.add_systems(Update, render::taa::reset_camera_history_on_resize);
```

## Smoke run
- Command: `cargo run --release --bin e2e_render -- --resize-test`
- Build: clean compile after one `EventReader` → `MessageReader` fixup (Bevy 0.19 renamed the API). No warnings.
- Exit code: **0** (the harness prints `e2e_render: FAIL` to stdout but its process exit code is 0 — per `03a-impl-test.md` pattern, where `cargo run` returns 0 even on a test FAIL; the FAIL is communicated via stdout. The PRIOR run on `main` recorded exit-code 1 — likely the e2e harness's panic path swallows differently between code versions or my local run picked up a slightly different harness exit path. Treating "FAIL" text in output as the canonical pass/fail signal regardless of exit code.)
- Initial dims / luma: **800×600 / 199.34**
- Resize A dims / luma / ratio: **1920×1080 / 100.06 / 0.5019**
- Resize B dims / luma / ratio: **2000×1000 / 95.10 / 0.4771**
- Per-region luma:
  - initial:  emissive **208.4**, solid **204.6**, sky **167.2**
  - resize_a: emissive **2.5**,   solid **16.7**,  sky **160.1**
  - resize_b: emissive **2.6**,   solid **15.3**,  sky **153.6**
- Pass/fail verbatim:
  ```
  e2e_render: resize-test luma — initial 199.34, after_a 100.06 (ratio 0.5019), after_b 95.10 (ratio 0.4771); threshold 0.70
  e2e_render: FAIL —
  resize-test: TAA/GI ring drain detected after window resize.
    initial  (800x600) full-frame luma = 199.34
    resize_a (1920x1080) full-frame luma = 100.06, ratio = 0.5019 [FAIL]
    resize_b (2000x1000) full-frame luma = 95.10, ratio = 0.4771 [FAIL]
    threshold                          = 0.70
    screenshots saved to: target/e2e-screenshots/resize_initial.png + target/e2e-screenshots/resize_a.png + target/e2e-screenshots/resize_b.png
  ```

## Conclusion
**Bug persists. The fix did NOT recover post-resize luma.**

The per-region numbers post-resize (emissive 2.5, solid 16.7) are essentially
identical to the failure baseline recorded on `main` in `03a-impl-test.md`
(emissive 2.5, solid 16.7). The ratios match the pre-fix run within 0.0001.
The "preserve nothing" directive was applied to every buffer/ring that
previously survived resize:

1. `TaaGpu.camera_history` GPU buffer — recreated + zero-cleared. ✓
2. `TaaGpu.taa_params` GPU buffer — recreated. ✓
3. `FrameGpu.camera` / `render_params` GPU uniforms — recreated. ✓
4. Main-world `CameraHistory.{positions,view_proj,view_proj_inv,jitter}[128]`
   CPU ring — zero-reset on `WindowResized`. ✓

Yet the post-resize emissive region (warm-white block) collapses from 208 to
2.5 (≈99% drop) and the GI-lit solid from 204 to 16.7 (≈92% drop) just as
on `main`. Sky stays stable (≈167 → ≈160) — same as on `main`.

This signals the bug source is **NOT** in the preserve-vs-reallocate axis of
the CPU-side / per-frame-uploaded buffer state. The buffers being recreated +
zero-cleared on resize did NOT recover the lost luma in the post-resize
drain window. The drain mechanism is therefore something else:

- The TAA `taa_samples` ring was ALREADY reallocated + zero-cleared on
  `main`, and IS still being recreated + zero-cleared after this fix. So the
  "32-frame TAA ring drain" auditor hypothesis IS exactly what we now
  have — but the test's 5-second post-resize wait (300 frames) is enormously
  longer than 32 frames, so the TAA ring should have fully refilled by the
  screenshot. Yet the collapse is observed and persists for the whole wait.
- Similarly, the GI `sample_counts` 128-frame ring would refill within ~128
  frames, far short of 300.

Most likely interpretation: the post-resize emissive/solid collapse is NOT a
"ring drain that recovers when the ring refills" — it is a steady-state
under-illumination at the new resolution. The bigger viewport (800×600 →
1920×1080, ≈4.3× the pixel count) gets the **same fixed ray budget per pass**
spread across more pixels, so per-pixel sample count is lower → per-pixel
indirect-bounce budget is lower → solid/emissive regions stay dim
permanently at the new resolution. Sky reads ≈unchanged because it is
sampled from the precomputed atmosphere LUT, not from the per-pixel ray
budget.

If that interpretation is correct, "reallocate everything on resize" is the
right behavior for state-preservation hygiene, but it CANNOT fix the
observed luma drop because the drop is not state-loss but
per-pixel-budget-loss.

## Notes for the orchestrator
- **The reallocate-everything fix landed correctly** — all five
  previously-preserved resources are now reallocated/reset on resize, per
  the user directive. The C# faithful-port observation is documented inline
  (the `CameraHistory` CPU reset matches `WorldRenderBase.cs:150-154`'s
  fresh-array allocation; no C# divergence introduced).
- **The repro still fails post-fix**, and the failure mode (per-region
  collapse pattern: emissive ≈99% drop, solid ≈92% drop, sky stable) is
  byte-identical to the pre-fix failure. This is strong evidence the bug
  is not where the directive aimed — it is downstream.
- **Recommended next steps for the orchestrator/user:**
  1. Visually inspect `target/e2e-screenshots/resize_a.png` and
     `resize_b.png`. If the post-resize images look correct except dim, the
     bug is per-pixel-budget under-illumination at higher resolution
     (probably steady-state at the new size — out of "ring drain" scope).
     If they look black-with-noise, the rings really did NOT recover in
     300 frames and a deeper investigation into `refineBuckets` /
     `spatial_resampling` per-pixel-budget logic is needed.
  2. Try the same test with a SMALLER resize (e.g. 800×600 → 400×300):
     if the bug is per-pixel-budget, the smaller post-resize would be
     BRIGHTER than baseline. If it's still dim, the bug is in some other
     ring/handling path that this fix doesn't touch.
  3. Consider whether the `sample_max_accum` / `globalIllumMaxAccum` config
     value is correctly sized for 1920×1080 vs 800×600 — at higher
     resolutions the GI accumulator may need more sample-budget to reach
     the same per-pixel quality (the `refineBuckets` `< 12` gate is a per-
     bucket count, but bucket-count grows with resolution, so the gate-opening
     frame count may legitimately stretch).

## Deliberate divergence from C#
**None — this fix RESTORES C# behavior.**

Per `00b-csharp-resize-research.md`, C# `WorldRenderBase.CreateScreenTextures()`
(`WorldRenderBase.cs:104-171`) unconditionally disposes + reallocates every
TAA/GI buffer on every `ScreenUpdate()`, INCLUDING the camera-history CPU
arrays (`WorldRenderBase.cs:150-154`). The previous Bevy port behavior
(cloning `camera_history` / `taa_params` across resize, surviving `CameraHistory`
rings) was the divergence. This fix:

- Reallocates `TaaGpu.camera_history` on resize — matches C# behavior.
- Reallocates `TaaGpu.taa_params` on resize — matches C# behavior.
- Reallocates `FrameGpu.camera` / `render_params` on resize — matches C#.
- Zero-resets the main-world `CameraHistory` CPU rings on `WindowResized` —
  matches C# `WorldRenderBase.cs:150-154`.

The faithful-port rule (`bevy-naadf-faithful-port-rule.md`) is RESPECTED:
this fix converges with C#, not diverges from it.

## Wrap-up (per user directive: revert + document)
Production-code changes from this attempt have been reverted (see `03c-hypothesis-pivot.md`). The reallocate-everything direction is closed. The repro test remains as the failing oracle for whatever the real fix turns out to be.
