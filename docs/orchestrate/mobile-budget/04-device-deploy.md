# mobile-budget — device deploy report

**Date:** 2026-05-21  
**APK:** `android/app/build/outputs/apk/debug/app-debug.apk` (414 MiB, commit `6c6977dd`)  
**Device:** Galaxy Tab A8 (SM-X205), Mali-G52 MP2 (Unisoc T618), Android 12

---

## Device + install

- **Device serial:** `adb-R9YT60YN5EL-Q8hSzx._adb-tls-connect._tcp` (wireless ADB / TCP)
- **Install result:** `Success` (`adb install -r -t ...app-debug.apk`)

---

## Budget log line

The expected `[budget]` line **did NOT appear** in logcat — neither the success path
(`bevy::log::info!` in `log_budget_decision`) nor the None-path warning
(`[budget] probe_limits returned None …`) was emitted.

`naadf` / `RustStdoutStderr` lines that DID appear (selection):

```
05-21 17:13:28.854 I/RustStdoutStderr( 8903): AdapterInfo { name: "Mali-G52", driver: "Mali-G52",
    driver_info: "v1.r34p0-01eac0.ab7ff64a33c26144175106a0ec40fedb", backend: Vulkan, … }

05-21 17:13:29.238 I/RustStdoutStderr( 8903): NAADF default scene embedded in fixed world:
    small=4×2×4 chunks centered at chunk-(46,0,46); fixed 96×32×96 chunks
    (1536×512×1536 voxels) with full-area ground at chunk-Y=0; …
    GPU producer disabled, CPU upload path active.

05-21 17:13:29.594 I/RustStdoutStderr( 8903): vox-gpu-rewrite Q4 instrumentation —
    device.limits().max_storage_buffer_binding_size = 268435456 B (256 MiB);
    allocated chunks = 2359296 B (2 MiB), blocks = 75497472 B (72 MiB),
    voxels = 150994944 B (144 MiB).

05-21 17:13:34.053 I/RustStdoutStderr( 8903): ERROR bevy_render::error_handler:
    Caught rendering error: Validation Error
    Caused by:
      In Device::create_bind_group, label = 'naadf_global_illum_bind_group'
        Buffer binding 4 range 294912000 exceeds `max_*_buffer_binding_size` limit 268435456

05-21 17:13:34.055 I/RustStdoutStderr( 8903): ERROR bevy_render::error_handler:
    Caught rendering error: Validation Error
    Caused by:
      In Device::create_bind_group, label = 'naadf_sample_refine_bind_group'
        Buffer binding 6 range 294912000 exceeds `max_*_buffer_binding_size` limit 268435456

05-21 17:13:34.788 I/RustStdoutStderr( 8903): ERROR bevy_render::error_handler:
    Quitting the application due to Validation RenderError
```

---

## Stability

- **`adb devices` at end:** device still connected (no reboot).
- **`io.naadf.bevy` process alive?** Yes — `ps -A` shows PID 8903, state `S`
  (sleeping / background shell; the Bevy app itself exited cleanly at `Quitting the
  application`; Android's GameActivity wrapper process lingers).
- **Focused app:** `com.android.settings/com.android.settings.Settings` (naadf quit,
  system returned to Settings).
- **FATAL / SIGSEGV / SIGABRT:** None. Clean `Validation RenderError` exit, no kernel crash.
  `lowmemorykiller` events (signal 9) hit unrelated system services under OOM pressure,
  but those are normal Android background-app culls, not caused by our process.

---

## Verdict

**FAIL** — `[budget]` line did NOT appear. The app crashed at first render frame with a
`Validation RenderError` on `naadf_global_illum_bind_group` binding 4 and
`naadf_sample_refine_bind_group` binding 6, both reporting `range 294912000 exceeds
limit 268435456`.

---

## Root-cause analysis (for next session)

The failure is a **TAA depth mismatch**: the buffer that exceeded the cap was
`294912000` bytes = `1920 × 1200 × 16 × 8` bytes, which is `taa_ring_depth = 16` at the
real screen resolution — not the expected `8` from the budget selection.

**What DID work:**
- `EffectiveWorldSize` was applied correctly: the world installed at `96×32×96 chunks`
  (= `(6, 2, 6)` segments), and the Q4 diagnostic confirmed `voxels = 144 MiB, blocks = 72 MiB`,
  both under the 256 MiB cap.

**What did NOT work:**
- `TaaRingConfig.depth` in the render sub-app ended up as **16** instead of **8**.
- The `[budget]` log line was never emitted — `log_budget_decision` (or the None-path
  `warn!`) either fired before the tracing subscriber was active (the probe's
  `MinimalPlugins::LogPlugin` installs the global subscriber inside `app.finish()`;
  if `log_budget_decision` fires before `app.finish()` has run on the probe, no subscriber
  is in place), or the probe ran with a different wgpu Limits value than the real device
  (surfaceless Vulkan device creation on this Mali driver may return a different cap in a
  probe context vs. the real app).

**Most plausible hypothesis:** `probe_limits()` runs successfully but the probe's wgpu
Vulkan device (created without a GameActivity surface, before `onSurfaceCreated`) reports
a `max_storage_buffer_binding_size` cap larger than 256 MiB — possibly the spec-default
uncapped value (`u64::MAX >> 1`) or a driver-reported larger value for a surface-less
headless device. If the probe cap is large, `select_budget()` walks the world-size ladder
and eventually falls to `(6, 2, 6)` (because the budget arithmetic for worlds `(16,2,16)`,
`(12,2,12)`, `(8,2,8)` pass the headroom check at a large cap, but the world ladder is
iterated from largest to smallest and the first that fits ALL three bindings — voxels,
blocks, chunks — plus any TAA depth wins). A cap around 512-767 MiB would cause
`select_budget` to pick world `(8,2,8)`, not `(6,2,6)`, so this hypothesis has a
contradiction unless the driver reports something in the 341-512 MiB range for surface-less
probes (no cap value simultaneously produces world `(6,2,6)` AND `taa_ring_depth = 16`
via `select_budget` — see arithmetic in Side notes §2 below).

**Alternative hypothesis (simpler):** `probe_limits()` returns `None` because the wgpu
adapter enumeration fails without an ANativeWindow surface pointer on this driver. The
`None` path in `probe_and_select()` falls back to `DEFAULT_TAA_RING_DEPTH = 32` and
`WORLD_SIZE_IN_SEGMENTS = (16, 2, 16)`. The `[budget] probe_limits returned None`
`warn!` fires before `MinimalPlugins::LogPlugin` installs the subscriber — lost. But then
`EffectiveWorldSize::from_segments((16, 2, 16))` would be inserted, not `(6, 2, 6)`.
The world IS `(6, 2, 6)`. So `probe_limits()` did NOT return `None`. This alternative is
ruled out by the world evidence.

**Most actionable next step:** add an `android_log` (bypass `bevy::log`) print at the very
first line of `android_main()` before `probe_and_select()` is called, using
`android_logger` or a direct `__android_log_print` FFI call, to capture what `caps`
reports BEFORE the subscriber question matters. Also log the probe device's actual
`max_storage_buffer_binding_size` via a direct `android_log` write inside `probe_limits()`
before returning — this will reveal whether the probe's device returns a different cap.

---

## Side notes / observations / complaints (REQUIRED)

1. **The world budget DID land correctly: `(6, 2, 6)` was applied, voxels=144 MiB,
   blocks=72 MiB.** This is the most important result: half the budget machinery is
   working. Only the TAA half is broken.

2. **No `select_budget()` input can simultaneously produce world `(6,2,6)` AND
   `taa_ring_depth = 16`.** Here is the arithmetic:
   - World `(6,2,6)` is selected only when world `(8,2,8)` fails the voxels-headroom
     check: `voxels(8,2,8) = 256 MiB > headroom`. This requires `headroom < 256 MiB`,
     i.e. `cap < 341.3 MiB`.
   - Depth 16 is selected only when `3 MP × 16 × 8 = 384 MiB ≤ headroom`, i.e.
     `headroom ≥ 384 MiB`, i.e. `cap ≥ 512 MiB`.
   - These two caps (`< 341.3 MiB` AND `≥ 512 MiB`) are mutually exclusive.
   - Therefore, either `EffectiveWorldSize` and `TaaRingConfig.depth` were set by
     **two different runs** of `select_budget` with different inputs, or one of them
     was overwritten by a path that `select_budget` does not control.

3. **The TAA buf size exceeds the cap by `294912000 / 268435456 = 1.10×` — only 10% over.
   `294912000 = 1920 × 1200 × 16 × 8`. If TAA depth had been 8 as intended:
   `1920 × 1200 × 8 × 8 = 147456000 bytes = 140.6 MiB` — well under the cap. The fix,
   once the root cause is confirmed, is surgical: get `TaaRingConfig.depth = 8` to land
   in the render sub-app.**

4. **The `app_args.rs` pin test `default_taa_ring_depth_is_a_supported_lever_value`
   asserts `matches!(depth, 16 | 24 | 32)`.** This test does NOT fail with `depth = 8`
   at runtime (it's a unit test on the default value, not an Android runtime assert), but
   the comment at line 39 of `app_args.rs` — "16 / 24 are the VRAM-lever alternatives" —
   predates `budget.rs` adding depth 8 and 4 to `TAA_RING_DEPTH_LADDER`. That pin test
   will need updating to include 8 and 4 once the bug is fixed and the intent is
   confirmed. The test currently passes only because `AppArgs::default()` returns depth=32.

5. **Shader assets missing from the APK** — 15 `Path not found: shaders/*.wgsl` errors
   appear at 17:13:29. These are the NAADF render shaders. This is surprising given that
   `app/build.gradle` has the `applicationVariants.configureEach` asset-copy block. The
   missing shaders would have caused rendering to fail even if the TAA buffer size were
   correct. This may be a second independent failure. If the budget bind-group error
   is fixed first and the app reaches shader compilation, these path errors will also
   need to be resolved. (The previous minimal-probe build presumably did not hit this
   because DefaultPlugins-only doesn't load NAADF shaders.)

6. **OOM pressure from system services** — `lmkd` killed `com.samsung.android.beaconmanager`
   and `com.google.android.ext.services` under memory pressure during the launch. Our
   app (121 MiB RSS, 6.5 GiB VSS — likely virtual mapping inflation from the 414 MiB APK)
   is squeezing system services. The device did NOT reboot, which is an improvement over
   the pre-budget OOM-reboot. The fix of the TAA depth + shader path should allow the
   renderer to actually start, at which point we can measure real RSS.

7. **Wireless ADB** — the device is on wireless ADB (`_adb-tls-connect._tcp`), not USB.
   This works fine but means USB debugging was enabled and the tablet was connected via
   TCP/IP rather than cable. No impact on the deploy procedure.

8. **Process lingers after app exit** — PID 8903 is still in `ps -A` after `app.run()`
   returns, in state `S`. Android's GameActivity keeps the process alive briefly after the
   Bevy app exits. This is expected; it is not a zombie or a crash hang.
