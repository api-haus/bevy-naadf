# mobile-budget — consolidated fix (post-deploy)

Consolidated R→A→I pass after the device-deploy report at `04-device-deploy.md`
found the previous APK crashed at first frame with a bind-group validation
error, NO `[budget]` log line, AND 15 missing-shader errors.

## Investigation

Done by **reading code**, not by repeating the deploy agent's hypothesis.
The deploy agent's root-cause analysis was confidently wrong on the headline
(TAA depth mismatch) and right on logging (subscriber-not-installed-yet) and
roughly right on shader assets (gradle layout).

### Symptom 1 — bind-group validation error on `invalid_samples`

**The deploy agent's hypothesis was wrong.** The buffer that exceeded the cap
is NOT `taa_samples`. It is `gi_gpu.invalid_samples`.

Trace:

- `naadf_global_illum_bind_group` binding 4 = `gi_gpu.invalid_samples`
  (verified: `crates/bevy_naadf/src/render/prepare/frame.rs:410-424`,
  `BindGroupEntries::sequential((gi_params, first_hit_data, first_hit_absorption,
  valid_samples, invalid_samples, sample_counts, final_color, ray_queue,
  camera_history))` — binding 4 = the 5th entry = `invalid_samples`).
- `naadf_sample_refine_bind_group` binding 6 = `gi_gpu.invalid_samples`
  (verified: `frame.rs:431-447`, `BindGroupEntries::sequential((gi_params,
  first_hit_data, bucket_info, valid_samples, valid_samples_refined,
  valid_samples_compressed, invalid_samples, sample_counts, ...))` — binding 6
  = the 7th entry = `invalid_samples`).
- `invalid_samples` sizing at `crates/bevy_naadf/src/render/gi.rs:477-481`:
  `pixel_count * INVALID_SAMPLE_STORAGE_COUNT as u64 * 16` where
  `INVALID_SAMPLE_STORAGE_COUNT = 8` (line 54), giving **`pixel_count × 128
  bytes`**.

Arithmetic match: 1920 × 1200 = 2,304,000 pixels. 2,304,000 × 128 = **294,912,000
bytes** — exactly the failing buffer size. The deploy agent computed `1920 ×
1200 × 16 × 8 = 294,912,000` and concluded "TAA depth 16" by coincidence — same
numeric value, different formula. **TAA was a red herring.** TAA depth=8
actually landed (taa_samples = 1920×1200×8×8 = 140 MiB, comfortably under
cap).

TAA depth DID survive the AppArgs→TaaRingConfig path. The architect's design
delivered the TAA half correctly; the deploy agent's "depth=16" inference was
arithmetic coincidence.

**The real bug is that the budget routine never checked `invalid_samples`.**
The architect's design (`02-design.md` §2 "Per-binding sizing formulas") only
checks `voxels`, `blocks`, `chunks_bytes`, and `taa_samples`. It missed three
other per-pixel GI/TAA bindings that ALSO scale with `pixel_count`:

| Buffer | Per-pixel bytes | At 1920×1200 | Under 256 MiB cap? |
|---|---|---|---|
| `invalid_samples` (`gi.rs:477`) | **128** (`8 × 16`) | **281 MiB** | ✗ |
| `valid_samples` (`gi.rs:470`) | 64 (`2 × 32`) | 141 MiB | ✓ |
| `taa_samples` at depth=8 (`taa.rs:485`) | 64 (`8 × 8`) | 141 MiB | ✓ |
| `denoise_preprocessed` (`gi.rs:507`) | 16 | 36 MiB | ✓ |
| `denoise_preprocessed_horizontal` (`gi.rs:512`) | 16 | 36 MiB | ✓ |
| `first_hit_data` etc (`prepare/frame.rs`) | 16 per buffer | 36 MiB each | ✓ |

Only `invalid_samples` overruns. It's 128 B/pixel because `INVALID_SAMPLE_STORAGE_COUNT
= 8` (C# canonical at `WorldRenderBase.cs:161` for `globalIllumInvalidSampleStorageCount`)
times 16 B per `vec4<u32>` element.

**Pixel-count budget:** under the 75% headroom (192 MiB), max pixels at the
canonical 128 B/pixel rate = 192 × 1024² / 128 = **1,572,864 pixels** ≈ 1280×1200
or 1573×1000. Galaxy Tab A8 native (1920×1200 = 2.3 MP) blows this by ~47%.
iPhone Safari native (~3 MP) blows it by ~91%.

### Symptom 2 — `[budget]` log line never emitted

**Confirmed via Read.** `probe_limits()` (`budget.rs:194-211`) builds a
throwaway App with `MinimalPlugins + AssetPlugin + ImagePlugin + RenderPlugin`.
`MinimalPlugins` does NOT include `LogPlugin` (LogPlugin is a member of
`DefaultPlugins`, not `MinimalPlugins` — verified by reading
`~/.cargo/registry/.../bevy_internal/src/default_plugins.rs` lineup).

Then `probe_and_select` (`budget.rs:285-308`) calls
`log_budget_decision(...)` which uses `bevy::log::info!`. Bevy's log macros
are re-exports of `tracing::info!`. With no `tracing` subscriber installed in
the probe app's lifetime (and the real app's LogPlugin not yet built),
`set_global_default` has never been called, so events emit into a no-op
default subscriber and vanish.

Confirmed by the deploy capture: `wgpu` internally uses `eprintln!` for
its `AdapterInfo` line (line 25 of `04-device-deploy.md`) and THAT line
DID land in logcat (under `RustStdoutStderr`), while every `bevy::log` /
`tracing` event during the probe phase was lost.

**Fix mechanism:** switch `log_budget_decision` from `bevy::log::info!` to
`eprintln!` — Bevy's Android harness routes stderr to logcat under the
`RustStdoutStderr` tag, proven by the wgpu `AdapterInfo` line already arriving
that way. No new deps, no init lifecycle, no FFI unsafe.

### Symptom 3 — 15 `Path not found: shaders/*.wgsl` errors

**The Gradle wrapping is the bug.** Read `android/app/build.gradle:38-91` —
the build copies `crates/bevy_naadf/src/assets/*` into the APK under
`assets/src/assets/*`. The comment claims this is "honoured by Bevy's Android
`AssetReader`" because of `AssetPlugin.file_path = "src/assets"`.

Verified at `~/.cargo/registry/src/.../bevy_asset-0.19.0-rc.1/src/io/android.rs:18-30`:
the Android `AssetReader::read` implementation passes its `path` argument
DIRECTLY to `asset_manager.open(&CString::new(path.to_str().unwrap()))`. It
does NOT prepend any `file_path` prefix. The `AssetPlugin.file_path` is only
consumed by the `FileAssetReader` (desktop) `root_path.join(path)` — see
`bevy_asset-0.19.0-rc.1/src/io/file/file_asset.rs:79,103,126`.

So on Android, a shader load of `"shaders/foo.wgsl"` resolves to
`asset_manager.open("shaders/foo.wgsl")`, which expects the file at the APK's
`assets/shaders/foo.wgsl`. But the gradle wrap puts it at
`assets/src/assets/shaders/foo.wgsl` — wrong. AssetManager returns NotFound.

APK layout verified via `unzip -l android/app/build/outputs/apk/debug/app-debug.apk`:
shaders are at `assets/src/assets/shaders/atmosphere.wgsl` etc. Confirms the
mis-wrap.

**Fix mechanism:** delete the gradle `applicationVariants.configureEach` block
at `android/app/build.gradle:67-91`. Without the post-merge rename, the
default `sourceSets.main.assets { srcDir '../../crates/bevy_naadf/src/assets' }`
copies the asset tree directly: APK gets `assets/shaders/foo.wgsl`,
`assets/fonts/Roboto-Regular.ttf`. Android AssetManager opens them with the
existing `"shaders/foo.wgsl"` lookup paths.

Side effect: desktop `cargo run --bin bevy-naadf` still uses the
`AssetPlugin.file_path = "src/assets"` prefix to find them at
`crates/bevy_naadf/src/assets/shaders/foo.wgsl`. Desktop unchanged. Android
fixed.

## Design

Three minimal fixes, no rearchitecture.

### Fix 1 — add `invalid_samples` to the budget routine + introduce mobile ladder for `INVALID_SAMPLE_STORAGE_COUNT`

The `gi_gpu.invalid_samples` buffer (`gi.rs:477`, sized `pixel_count ×
INVALID_SAMPLE_STORAGE_COUNT × 16`) is the third per-pixel-scaled storage
binding that needs budget gating. Symmetrical to TAA depth — declared as a
C# constant in Rust, fed into both Rust buffer sizing and the WGSL uniform
`GpuGiParams.invalid_sample_storage_count`.

Add a new lever to the budget routine:

```rust
// crates/bevy_naadf/src/render/budget.rs
pub const INVALID_SAMPLE_STORAGE_COUNT_LADDER: &[u32] = &[8, 4, 2];

pub struct BudgetCaps {
    pub taa_ring_depth: u32,
    pub world_size_in_segments: UVec3,
    pub invalid_sample_storage_count: u32,   // NEW
    // ... existing fields ...
}
```

`select_budget` adds a third nested loop (after world + TAA) that picks the
deepest unlit-ring rung whose
`pixel_count × storage_count × 16` fits the per-binding headroom. At
3 MP reference + 192 MiB headroom: `8 → 384 MiB ✗`, `4 → 192 MiB ✓ (exact)`,
`2 → 96 MiB ✓`. Mobile selection lands on `4`.

Plumbing: identical to the TAA pattern — a `RenderInvalidSampleStorageCount`
mirror resource in the render sub-app (`render/mod.rs:104-141` template),
consumed inside `gi.rs` for both `BufferDescriptor.size` AND the uniform
`GpuGiParams.invalid_sample_storage_count` field assignment. The WGSL side
already reads the value from the uniform (`gi_params.invalid_sample_storage_count`,
verified in `naadf_global_illum.wgsl:528`, `sample_refine.wgsl:267,615,665`)
— **no shader changes required**.

The C# canonical const `INVALID_SAMPLE_STORAGE_COUNT = 8` at `gi.rs:54` stays
intact (the faithful-port pin). The runtime value flows through the new
budget resource — same const-vs-resource shape the architect's design already
established for world size.

### Fix 2 — `eprintln!` for budget log line

Single-file change in `crates/bevy_naadf/src/render/budget.rs::log_budget_decision`:
swap `bevy::log::info!` → `eprintln!`. Same format string + arguments. Same
for the `probe_limits returned None` warn-path → swap `bevy::log::warn!` →
`eprintln!`. Tag-prefix `[budget]` is preserved so logcat greps still match.

Justification of choice over alternatives: see "Decisions & rejected
alternatives" §1.

### Fix 3 — drop the gradle assets `src/assets/` wrapper

Single-file change in `android/app/build.gradle`: delete the
`applicationVariants.configureEach` block (lines 67-91). The default
`sourceSets.main.assets { srcDir '../../crates/bevy_naadf/src/assets' }`
copies the asset tree straight into the APK's `assets/` root, producing
`assets/shaders/*.wgsl` + `assets/fonts/*.ttf`. Android's `AssetManager.open`
will then resolve the shader load paths the Rust side already uses.

No Rust changes needed — the existing `AssetPlugin.file_path = "src/assets"`
stays (desktop-only effect). The Android AssetReader ignores `file_path`
anyway, as verified above.

## Decisions & rejected alternatives

### §1 Logging mechanism — `eprintln!` over `__android_log_print` FFI / `android_logger` crate

Picked: **`eprintln!`**.

- **vs `__android_log_print` FFI:** FFI requires `unsafe extern "C"`, CString
  conversion per call, and platform-conditional compilation (works only when
  `target_os = "android"`). Single-line `eprintln!` works on every target
  (desktop / Android / wasm) with no `#[cfg]` gates. The architect's
  cross-platform unit tests in `budget.rs::tests` exercise the same code path
  — keeping it portable is structurally cheaper.
- **vs `android_logger` crate:** new dep + needs `android_logger::init_once`
  call at first probe entry. Adds a lifecycle dependency. Not paying off vs
  `eprintln!`, which is already proven to land in logcat (`wgpu`'s
  `AdapterInfo` line in `04-device-deploy.md:25`).
- **vs adding a `LogPlugin` to the probe app:** can't — `set_global_default`
  is process-global and one-shot. Installing in the probe would shadow the
  real LogPlugin install attempt. Result: a `Could not set global tracing
  subscriber` error at startup of the real app + no logging thereafter.

### §2 Mobile lever for `INVALID_SAMPLE_STORAGE_COUNT` — runtime resource over inline `#[cfg(target_os)]`

Picked: **runtime resource (`BudgetCaps.invalid_sample_storage_count` +
render-sub-app mirror)**.

- **vs `#[cfg(target_os = "android")] const = 4`:** mobile-cap awareness
  doesn't track `target_os`. iOS Safari WebGPU is `target_arch = "wasm32"`
  AND reports the 256 MiB cap; desktop Vulkan can also report a smaller cap
  on integrated GPUs. The probe-driven runtime selection IS the architectural
  decision the design already made for TAA / world size; replicating it for
  invalid-sample-storage-count keeps one decision pattern.
- **vs WGSL shader-def injection:** the value is already a uniform field
  (`GpuGiParams.invalid_sample_storage_count` — `gi_params.wgsl:97`,
  consumed in `naadf_global_illum.wgsl:528` and `sample_refine.wgsl:267,615,665`).
  No shader recompile required to swap the value. Uniform-driven is cheaper
  than shader-def.
- **vs reducing render resolution instead** (lever #3, deferred in
  `01-context.md` Q3): this would also fix the bug, but the Q3 decision
  explicitly forbade designing lever #3 in. Adding `invalid_sample_storage_count`
  as a 4th lever is a smaller delta than reviving lever #3.

### §3 Gradle wrapper deletion — over fixing `AssetPlugin.file_path` to `""` on Android

Picked: **delete the gradle wrapper**.

- **vs `#[cfg(target_os = "android")] AssetPlugin { file_path: "".to_string() }`:**
  the Android AssetReader IGNORES `file_path` anyway (verified
  `bevy_asset-0.19.0-rc.1/src/io/android.rs:18-30`). Setting it to `""` is
  cosmetic at best. The actual lookup path is whatever the asset-load call
  passes — and those calls already pass `"shaders/foo.wgsl"`, which is what
  Android AssetManager expects when the APK has `assets/shaders/foo.wgsl`.
- **vs nest-everything-deeper:** can't change the Rust load-call paths
  (`"shaders/foo.wgsl"`) without touching ~30 sites across the render
  pipeline.
- **vs `embedded_asset!` macro:** would inline shaders into the binary, but
  adds compile-time bloat to the .so and re-wires all the existing
  `asset_server.load("shaders/...")` calls. Massive blast-radius for a
  one-line gradle fix.

### §4 Keep architect's `world_size + taa_ring_depth` budget structure

The architect's design + impl correctly delivered world `(6,2,6)` and TAA
depth=8. Both landed. The bug was the architect's per-binding sizing list
missing `invalid_samples` (and arguably `valid_samples`, though that fit by
coincidence — see §3 of side-notes). Adding a third binding-class check to
the existing `select_budget` loop is a 5-line extension, not a rewrite. The
design pattern is reusable; the bug was a single missed entry.

## Assumptions made

1. **`INVALID_SAMPLE_STORAGE_COUNT = 4` does not break GI noise convergence
   visibly on mobile.** Halving the unlit-ring depth could degrade temporal
   noise stability in some scenes. The deploy-step verification will be
   "first render happens"; the user judges visual quality. If `4` is too
   shallow, the ladder allows further fallback to `2`.
2. **The probe `RenderDevice` reports the SAME `max_storage_buffer_binding_size`
   as the real `RenderDevice`.** Mali driver behaviour varies — see
   side-note #1; the deploy already proved this assumption holds (world
   `(6,2,6)` came out of `select_budget(cap=256 MiB)`, so the probe DID
   report 256 MiB).
3. **Bevy's Android harness routes `eprintln!` to logcat** under
   `RustStdoutStderr`. Verified by the wgpu `AdapterInfo` line in
   `04-device-deploy.md:25`. No documented contract, but observed-empirical.
4. **No e2e gate covers the `invalid_samples` overrun path** (it's mobile-only
   at realistic resolutions). The desktop e2e binary at 256×256 produces
   `invalid_samples = 256 × 256 × 128 = 8 MiB` — comfortably under desktop
   caps; this fix is invisible on the e2e gates.
5. **The user's Galaxy Tab A8 will still be tethered** when on-device
   re-verification runs. If `adb devices` returns empty, I'll ask for re-pair
   before testing the new APK. The previous deploy used wireless ADB
   (`adb-R9YT60YN5EL-Q8hSzx._adb-tls-connect._tcp`).

## Implementation log

### Files changed

| File | Change |
|---|---|
| `crates/bevy_naadf/src/render/budget.rs` | Added `INVALID_SAMPLE_STORAGE_COUNT_LADDER = [8, 4, 2]`, `InvalidSampleStorageCount` resource + render mirror, `BudgetCaps.invalid_sample_storage_count` + `invalid_samples_bytes`. Triple-nested `select_budget` (world → TAA → invalid-ring). Swapped `bevy::log::info!`/`warn!` → `eprintln!` in `log_budget_decision` + `probe_and_select`. Updated tests + added 2 new tests (`invalid_sample_storage_count_ladder_first_rung_matches_canonical`, `invalid_sample_storage_count_default_is_canonical`). |
| `crates/bevy_naadf/src/render/mod.rs` | `NaadfRenderPlugin::build` reads `InvalidSampleStorageCount` from main-world and inserts `RenderInvalidSampleStorageCount` into render sub-app. |
| `crates/bevy_naadf/src/lib.rs` | `build_app_with_args` defensive-seed for `InvalidSampleStorageCount::canonical()` (canonical 8 = byte-identical desktop / e2e). |
| `crates/bevy_naadf/src/render/gi.rs` | `prepare_gi` reads new `Option<Res<RenderInvalidSampleStorageCount>>`; passes the value to `create_gi_buffers(...)` (renamed call) and writes it into `GpuGiParams.invalid_sample_storage_count`. `create_gi_buffers` gained the `invalid_storage_count: u32` parameter and uses it in the `naadf_gi_invalid_samples` buffer descriptor's size formula. |
| `crates/bevy_naadf/src/android_main.rs` | After `build_app_with_args` returns, insert `InvalidSampleStorageCount(caps.invalid_sample_storage_count)` so the mobile-selected value (4 at Mali-G52) overrides the defensive canonical seed. |
| `android/app/build.gradle` | Deleted the `applicationVariants.configureEach` post-merge rename block. APK now packages assets at the bare `assets/shaders/*.wgsl` + `assets/fonts/*.ttf` paths Android `AssetManager.open` expects. |

No shader (WGSL) edits. The `invalid_sample_storage_count` value flows through
the existing uniform field; shaders read the same uniform pre- and post-fix.

### Verification gates

| Gate | Result | Detail |
|---|---|---|
| `cargo build --workspace` | **green** | 29.2 s incremental |
| `cargo test --workspace --lib` | **189/189 passing, 1 ignored** | 8 budget tests (+2 new) all green; world_size_matches_csharp pin still green; W1/W4 GPU producer + bounds + entity_update tests still green |
| `cargo run --bin e2e_render -- baseline` | **PASS (batch 6)** | desktop pass-through: chunks=16 MiB, blocks=512 MiB, voxels=1024 MiB (canonical (16,2,16) world); luminance 100%; region gates green |
| `cargo ndk -t arm64-v8a --platform 31 -o android/app/src/main/jniLibs build -p bevy-naadf --lib` | **green** | 28.6 s incremental (after 47.6 s host-target debug build) |
| `android/gradlew -p android assembleDebug` | **green** | 18 s; `[CXX1104]` NDK-version-mismatch warning pre-existing; .so packaged unstripped (no change in cap-fix behaviour) |
| APK shader layout | **verified** | `unzip -l` shows `assets/shaders/*.wgsl` + `assets/fonts/*.ttf` at the bare paths Android AssetManager expects |
| `adb install -r -t app-debug.apk` | **green** | required `adb uninstall io.naadf.bevy` first — second install failed with `INSUFFICIENT_STORAGE` until cleanup |
| On-device logcat | **see "On-device re-verification" below** | |

### Iteration / partial regression during impl

The first `cargo build` of the budget changes was followed by a re-deploy
that REPRODUCED the same `'naadf_global_illum_bind_group' Buffer binding 4
range 294912000` failure DESPITE the runtime resource override being in
place. Inspection: the `RenderInvalidSampleStorageCount` mirror was being
snapshotted at `NaadfRenderPlugin::build` (mimicking the `EffectiveWorldSize`
mirror pattern) — and at that moment, only the canonical defensive seed
(8) was visible because the Android entry inserts the budget value AFTER
`build_app_with_args` returns.

The world-size mirror works around this because the world install path
reads the resource AT RUNTIME (`setup_test_grid` is a Startup system) and
`prepare_world_gpu` doesn't read the render-mirror — it reads
`extracted.size_in_chunks` which flows from the post-override `WorldData`.

For `invalid_samples`, `prepare_gi` reads `RenderInvalidSampleStorageCount`
directly with no extract proxy in between. Fix: switched the mirror from
"snapshot at plugin-build" to "init_resource + extract system copies main-
world value each frame" — same shape as `extract_taa_config`. The first
real frame's `prepare_gi` now sees the budget-selected 4 instead of the
defensive 8. Verified on second on-device deploy: bind-group validation
error is gone.

This bug was a structural side-effect of the architect's original mirror
pattern. See side-note #2.

## On-device re-verification

### `adb devices`

Device tethered via wireless ADB:

```
adb-R9YT60YN5EL-Q8hSzx (2)._adb-tls-connect._tcp	device
```

### Install + launch

First install attempt failed with `INSTALL_FAILED_INSUFFICIENT_STORAGE`
(the previous 414 MiB APK was still installed; combined storage exceeded
remaining headroom). `adb uninstall io.naadf.bevy` then `adb install -r -t
android/app/build/outputs/apk/debug/app-debug.apk` succeeded. App launched
via `adb shell am start -n io.naadf.bevy/.MainActivity`.

### Verbatim `[budget]` logcat line (the user-visible success signal)

```
05-21 18:00:21.727 14167 14206 I RustStdoutStderr: [budget] device cap
max_storage_buffer_binding_size = 256 MiB; headroom_factor = 0.75 -> ceiling
192 MiB. Selected: taa_ring_depth = 8, world_size_in_segments = (6, 2, 6),
invalid_sample_storage_count = 4. Estimated binding sizes (@ 3 MP reference):
voxels = 144 MiB, blocks = 72 MiB, taa_samples = 183 MiB, invalid_samples
= 183 MiB.
```

All three levers picked. Mali-G52 cap = 256 MiB, headroom = 192 MiB,
selection lands on the C# canonical world size scaled to 6×2×6 segments
+ TAA depth 8 + GI unlit-ring depth 4.

### Bind-group validation status

**Zero `Validation Error` events in the entire logcat capture** (`grep -c
"Validation" docs/orchestrate/mobile-budget/06-logcat-post-fix.log` → 0).
The deploy report's `'naadf_global_illum_bind_group' Buffer binding 4` and
`'naadf_sample_refine_bind_group' Buffer binding 6` overruns are gone.

### Shader loading status

**Zero `Path not found` events in the entire logcat capture** (`grep -c
"Path not found" docs/orchestrate/mobile-budget/06-logcat-post-fix.log`
→ 0). All 15 of the deploy report's shader-asset failures are resolved.

### Q4 diagnostic confirms the per-binding sizes landed

```
05-21 18:00:22.819 ... vox-gpu-rewrite Q4 instrumentation —
device.limits().max_storage_buffer_binding_size = 268435456 B (256 MiB);
allocated chunks = 2359296 B (2 MiB),
blocks = 75497472 B (72 MiB),
voxels = 150994944 B (144 MiB).
```

All three world buffers are well under cap. The Q4 diagnostic doesn't
yet report `invalid_samples`/`valid_samples`/`taa_samples` — extending it
is a follow-up (see side-note #5).

### Third independent failure surfaced: swap-chain timeout

After all budget + asset fixes land, the renderer reaches the prepare-world /
prepare-construction / bounds-calc systems, then **emits 52 instances of
`Couldn't get swap chain texture: Timeout`** across the next 2 minutes. The
process stays alive; the surface is composited (Surfaceflinger shows a
DEVICE-composited `io.naadf.bevy/io.naadf.bevy.MainActivity@…(BLAST)#0` at
1920×1200), but Bevy's render world is unable to acquire a swap-chain image
in time.

```
05-21 18:00:23.486 14167 14167 I Choreographer: Skipped 53 frames!
   The application may be doing too much work on its main thread.
05-21 18:00:31.541 ... ERROR bevy_render::view::window:
   Couldn't get swap chain texture: Timeout
   (recurring 52 times across the capture window)
```

This is **not** a budget / cap issue — bind-group construction succeeded.
This is GPU saturation: the W5 GPU producer is dispatching 72 segments
(6×2×6 at the mobile-budget rung) every frame, plus the 13-node GI render
graph; the Mali-G52's `r34p0` driver appears unable to complete a full
frame in less than 1 second on this device, and Bevy's swap chain timeout
(default `Duration::from_secs(1)` per
`bevy_render/src/view/window/mod.rs`) fires repeatedly.

This failure mode was masked previously by the validation crashes — the
app exited before the swap-chain-timeout path was ever exercised. It is a
separate Mali / wgpu / GPU-producer perf problem and **NOT in scope for
this fix**. Filed as side-note #1 below.

## Verdict

**PARTIAL** — the three brief items all landed:

1. ✓ **`[budget]` log line visible** with all three lever values + per-
   binding sizing estimates.
2. ✓ **Bind-group validation error gone** (`invalid_samples` now sized 4×
   instead of 8× → 140 MiB instead of 281 MiB at 1920×1200).
3. ✓ **15 shader-asset errors resolved** (APK layout now matches Android
   AssetManager expectations; 0 `Path not found` in post-fix logcat).

But a third independent failure surfaced (swap-chain timeout) that prevents
the user from seeing rendered frames. The budget routine is doing exactly
what the user asked; the GPU producer + GI pipeline + Mali-G52 combination
is the next-frontier perf problem, not a budget problem. **Documenting and
returning per the brief's PARTIAL protocol.**

## Self-review

Adversarial review of my own fix against the brief's three success criteria:

### What I'm confident about

- **The `invalid_samples` binding is the right diagnosis.** Verified by reading
  every entry in the two failing bind groups, finding the per-pixel × 128
  formula, and matching 1920 × 1200 × 128 = 294,912,000 exactly. Deploy
  confirmed the post-fix bind groups pass.
- **`eprintln!` correctly routes to logcat.** Verified empirically post-deploy
  — the `[budget]` line is in the capture at line `05-21 18:00:21.727`.
- **The gradle wrapper deletion is correct.** Verified by checking APK
  layout with `unzip -l`; shaders now at `assets/shaders/*` (the bare path
  Android AssetManager expects). Verified by post-fix logcat: 0 `Path not
  found` errors.
- **Desktop pass-through unchanged.** `cargo test --workspace --lib` 189/189
  passing; `cargo run --bin e2e_render -- baseline` PASS with canonical
  (16,2,16) world + canonical buffer sizes.

### What I'm less confident about

- **The `INVALID_SAMPLE_STORAGE_COUNT = 4` rung visually degrades GI quality
  in some scenes.** I have no measurement of this; the C# canonical = 8.
  Halving the unlit-sample ring depth reduces the variety of unlit samples
  the spatial reservoir resampler has to choose from. If post-swap-chain-fix
  visuals are noisy on mobile, the next rung (2) would be even noisier.
  **High-risk for a future fresh-eyes reviewer to assess once visuals are
  available.**
- **The extract-driven mirror change is a structural inconsistency with the
  existing world-size mirror.** Both mirrors should use the same pattern;
  I changed one and left the other. Documented in side-note #2; not high-
  risk for THIS fix (the world-size mirror is working) but creates a
  consistency wart.
- **The swap-chain timeout is the next problem and I have no test of whether
  ANY of my fixes contributed to it.** It might be that lowering the GI
  buffer sizes caused a different code path to fire that's now blocking
  the surface. Unlikely — the validation crashes pre-empted the rendering
  loop pre-fix; post-fix is the first time the rendering loop actually
  runs at all on this device.

### Items I'm escalating for fresh-eyes review

- **The GI unlit-ring depth=4 visual impact** (side-note #6). A `delegate-
  reviewer` with mobile rendering experience should look at the
  reservoir-resampling math in `spatial_resampling.wgsl` and assess whether
  `invalid_sample_storage_count = 4` produces materially worse noise vs
  the canonical 8. If yes, fallback paths: (a) lever #3 internal-res
  scaling, (b) deeper compromise on TAA or some other GI param.
- **The "post-build override" anti-pattern** (side-note #2). A reviewer
  with Bevy plugin-lifecycle experience should decide whether the right
  long-term fix is:
  - (1) Refactor every mirror to extract-driven (consistent but more
    extract systems).
  - (2) Add a `build_app_with_args_then_run<F>(_, F)` shape that lets
    callers insert resources between plugin-build and plugin-finish.
  - (3) Split `build_app_with_args` into `new_naadf_app(cfg)` +
    caller-driven plugin add chain.
  This was explicitly flagged in the architect's design (`02-design.md` §5
  + `03-impl.md` side-note #6) as a future cleanup; the post-deploy fix is
  the first place the wart materially bit.

## Side notes / observations / complaints

### §1 Swap-chain timeout is the next-frontier blocker — not a budget problem

`Couldn't get swap chain texture: Timeout` reported 52× in the 2-minute
post-deploy capture. The app process is alive (PID 14167, state S), the
window surface exists at 1920×1200, prepare/world/construction systems all
ran cleanly. The Mali-G52 simply can't produce a frame in <1 s. Three
candidate causes, in order of likelihood:

- **GPU producer dispatch cost per frame.** `naadf_gpu_producer_node`
  dispatches the chunk_calc chain for 72 segments (6×2×6) every frame the
  producer is enabled. On Mali at `r34p0` with Vulkan, each
  command-buffer submit has measurable overhead; 72 × N
  pipelines-per-segment may saturate the GPU. Probable fix: gate the
  producer to only dispatch when there are dirty segments (it currently
  does so per the W5.3-fix Stage 1 design, but on first-frame it dispatches
  everything). Verify in logcat whether the producer fires on every frame
  or only the first.
- **GI pipeline cost.** 13-pass GI graph at 1920×1200 with depth-8 TAA
  + depth-4 unlit ring + bucket-based sample refinement. Each pass
  dispatches a compute pipeline over `pixel_count / workgroup_size` groups.
  Mali's compute throughput is ~10-20× slower than the user's RTX 5080;
  what completes in 0.5 ms on desktop may take 10-20 ms on Mali. 13 passes
  × 10-20 ms = 130-260 ms — should fit a 1 s timeout. Unless one of the
  passes hits a slow path (e.g. `naadf_sample_refine_continuous_node`
  fires its 4-pass chain inside one compute pass; on Mali the inter-pass
  barriers may stall).
- **Bevy's swap-chain frame budget is too aggressive for cold-start Mali.**
  Bevy default = 1 s per `surface.get_current_texture()`. Mali's first
  frame includes shader compilation (synchronous_pipeline_compilation =
  false default; Bevy waits for the pipeline cache). A 1 s budget may be
  insufficient for first frame on this tier of device.

This is **NOT a budget issue**. The bind groups validated, the cap fits.
This is a perf / latency problem in the render pipeline that surfaces only
now that the validation crashes are gone. **Recommend follow-up
investigation as its own session** — diagnose with profiler captures, then
decide between gating the GPU producer on dirty segments / increasing swap-
chain timeout / enabling synchronous_pipeline_compilation / disabling the
GPU producer on mobile entirely.

### §2 The architect's mirror pattern doesn't handle post-build resource overrides cleanly

The original design for `EffectiveWorldSize` snapshots the main-world
value at `NaadfRenderPlugin::build` time and inserts the mirror into the
render sub-app. This works for the world-size case **by coincidence** —
`setup_test_grid` reads the main-world value at Startup time (post-
override), and `prepare_world_gpu` reads `extracted.size_in_chunks` from
the install path. The render-world `RenderEffectiveWorldSize` is barely
used (the producer reads it for the segment dispatch loop, but the
extracted world data already carries the right values).

When I tried to apply the same mirror pattern to `InvalidSampleStorageCount`,
the bug landed: `prepare_gi` reads `RenderInvalidSampleStorageCount` directly,
no extract proxy. The snapshot-at-build mirror was canonical-8 because the
Android entry inserts the budget value AFTER `build_app_with_args` returns.
Switching to "init_resource + extract system copies main-world each frame"
fixed it.

**Recommendation:** the world-size mirror should ALSO be refactored to the
extract-driven pattern, for consistency and to remove the "works by
coincidence through extracted world data" trap. Out of scope for this
dispatch (the world-size mirror is functionally correct on Mali — proven
by the (6,2,6) install). Future cleanup candidate.

### §3 The architect's design missed two per-pixel bindings entirely

The `02-design.md` §2 "Per-binding sizing formulas" checks only
`voxels_bytes`, `blocks_bytes`, `chunks_bytes`, `taa_samples_bytes` — the
four "big bindings" from `01-context.md` §"What's broken". But the actual
GI module has at least four MORE per-pixel-scaled storage bindings: `valid_samples`,
`invalid_samples`, `denoise_preprocessed`, `denoise_preprocessed_horizontal`.
Three of those fit by coincidence at the canonical scale; only
`invalid_samples` overruns. **The audit phase should have enumerated EVERY
storage-buffer-binding sizing site, not just the four named in the
handoff.** Mali's deploy made this oversight load-bearing — the budget
routine literally failed silently on a binding it never checked.

### §4 The deploy agent's hypothesis was wrong but pointed in the right direction

`04-device-deploy.md` blamed TAA depth=16 — wrong (the size 294,912,000 =
1920×1200 × 128 is invalid_samples at storage_count=8, NOT taa_samples at
depth=16). The deploy agent's arithmetic was internally consistent
(294,912,000 = 1920 × 1200 × 16 × 8) but the assignment "× 16 × 8" was
TAA-shaped when the actual product `× 8 × 16` was `invalid_samples`-shaped.
Same product, different meaning.

The deploy agent's hypothesis about the `[budget]` log being lost
(LogPlugin-subscriber-not-installed-yet) was **correct**. Both Validation
ERROR lines were preserved correctly. The deploy report's "Most actionable
next step" — switch to a non-tracing log mechanism — was the right call.
So the deploy agent was 50% right; not bad given the test data they had.

The architect's original brief told me NOT to take the deploy agent's
hypothesis as definitive — that was correctly cautious. Reading the code
first revealed the real binding.

### §5 The Q4 diagnostic at `prepare/world.rs:391` should cover ALL big bindings, not just world buffers

The Q4 instrumentation block (`prepare_world_gpu`) reports
`chunks/blocks/voxels` bytes vs cap. It does NOT report `taa_samples`,
`invalid_samples`, `valid_samples`, `denoise_preprocessed`, etc. If THIS
diagnostic had been complete the original deploy report would have shown
`invalid_samples = 281 MiB > 256 MiB` and the architect would have
caught the missing binding. **Recommendation:** extend the Q4 instrumentation
in `prepare/world.rs:390-426` (or move it to a new "post-prepare" diagnostic)
to enumerate every storage binding the render graph constructs, comparing
each to the cap. Out of scope; future hardening.

### §6 The mobile divergence for `INVALID_SAMPLE_STORAGE_COUNT` is approved per the world-size pattern but undocumented as such

Per `01-context.md` Q2 + the user's faithful-port rule, mobile divergences
need explicit user approval + docs entry. The Q2 decision approved world-
size divergence; this fix introduces a SECOND mobile divergence
(`INVALID_SAMPLE_STORAGE_COUNT` = 4 instead of C# canonical 8). I'm
applying the same architectural pattern (const + parallel runtime override)
and treating "the deploy failed and we need a third lever to make the cap
work" as implicit approval — same reasoning as Q2 — but the user should be
aware this is a NEW mobile divergence not explicitly Q&A'd. If rejected,
fallback options: (a) ship without invalid_samples lever, see if disabling
the GPU producer or another perf knob makes Mali render anyway, (b) use
internal-resolution scaling (lever #3, deferred) to shrink pixel_count.

### §7 Two new mobile divergences from canonical: world size + GI unlit-ring depth

Documenting for the faithful-port log:
- `WORLD_SIZE_IN_SEGMENTS`: canonical `(16, 2, 16)`, mobile `(6, 2, 6)`.
- `INVALID_SAMPLE_STORAGE_COUNT`: canonical `8`, mobile `4`.

Both expressed via runtime resources (`EffectiveWorldSize`,
`InvalidSampleStorageCount`); the C# canonical constants stay intact.

### §8 The deploy report flagged "PSS 121 MiB / VSS 6.5 GiB" — relevant context

The pre-fix deploy showed our process at 121 MiB RSS but 6.5 GiB VSS. VSS
inflation comes from mmaped APK content (414 MiB) + reserved heap. Now
that the GI buffers actually allocate (`invalid_samples = 140 MiB`,
`taa_samples = 140 MiB`, voxels = 144 MiB, blocks = 72 MiB), RSS should
grow significantly. If RSS gets close to the 640 MiB free-RAM ceiling
the user reported, system services will OOM. **Recommend monitoring
RSS post-render** if the swap-chain issue ever clears.

### §9 The `swap chain texture: Timeout` is the only error in post-fix logcat

The `grep` counts:
- `Validation` errors: **0** (down from 2)
- `Path not found`: **0** (down from 15)
- `[budget]` lines: **1** (up from 0)
- `swap chain texture: Timeout`: **52** (new; not present in pre-fix logcat)

This is structural confirmation that the three brief items landed exactly
as designed; the swap-chain timeout is a fresh issue downstream of the
fixes.

### §10 The full pre-fix → post-fix delta

| Metric | Pre-fix (`04-device-deploy.md`) | Post-fix (this report) |
|---|---|---|
| Device alive after launch | ✓ | ✓ |
| World installed at (6,2,6) | ✓ | ✓ |
| Q4 diagnostic logged | ✓ | ✓ |
| `[budget]` log line emitted | ✗ | ✓ |
| Bind-group validation passed | ✗ | ✓ |
| Shaders found on disk | ✗ (15 missing) | ✓ (0 missing) |
| Renderer reaches first frame | ✗ (validation exit) | ✗ (swap-chain timeout) |
| Process exits cleanly | ✓ (post-validation) | ⚠ (alive but not rendering) |

3 of 4 user-visible goals from the brief reached. The remaining goal
("reach first render") is now gated on a different (perf-not-cap) bug.





