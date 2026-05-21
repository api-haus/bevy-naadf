# android-build — handoff for scalable (V)RAM budget work

## Worktree context

- **Branch:** `feat/android-build`
- **Worktree:** `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/android-build`
- **Slug:** `android-build`
- **Status as of 2026-05-21:** APK build pipeline lands. Minimal-probe survives on a Mali-G52 tablet and dumps `wgpu::Limits` to logcat; the full Naadf world install OOM-reboots the same tablet. Next session designs scalable budgets so the full app can run on mobile (Android **and** iOS WebGPU — same caps).

## Why this exists

The C#-faithful 256×32×256 fixed-world container in `render/prepare/world.rs` unconditionally allocates four GPU buffers that exceed the WebGPU spec-minimum per-binding cap (256 MiB) by 2-4×. iOS Safari WebGPU refuses; the user's Galaxy Tab A8 (Mali-G52, native Vulkan) physically OOMs the kernel. The fix is **buffer-binding budget awareness at startup**, not a runtime workaround.

## Mali-G52 / Vulkan facts captured 2026-05-21

Mali-G52 MP2 (Unisoc T618), Android 12, wgpu 29.0.3 / Vulkan backend, unified memory:

| Limit | Value | Notes |
|---|---|---|
| `max_storage_buffer_binding_size` | **256 MiB** (268,435,456) | THE cap. Matches WebGPU spec minimum. Same on iOS Safari. |
| `max_buffer_size` | 2 GiB (2,147,483,647) | Buffer can be huge — only the bound *slice* is capped. |
| `max_uniform_buffer_binding_size` | 64 KiB | |
| `max_bind_groups` | 4 | Spec floor. |
| `max_storage_buffers_per_shader_stage` | 35 | |
| `max_compute_invocations_per_workgroup` | 384 | |
| Subgroup size | 8 (min == max) | `SUBGROUP` + `SUBGROUP_BARRIER` features both present. |
| Available features (selection) | `MAPPABLE_PRIMARY_BUFFERS`, `TEXTURE_ATOMIC`, `MULTIVIEW`, `SHADER_F16`, `SHADER_I16`, `TIMESTAMP_QUERY`, `ASTC`, `ASTC_HDR` | |
| Total system RAM | 2.5 GiB (shared CPU+GPU via Mali) | ~640 MiB available with stock Samsung firmware booted. |
| Empty-probe footprint (DefaultPlugins only) | 250 MiB PSS, 67 MiB GPU mtrack | Baseline overhead before any Naadf state. |

**Single most important fact: `max_storage_buffer_binding_size = 256 MiB` is the WebGPU spec mandatory minimum and what mobile drivers consistently report.** Treat 256 MiB as the universal mobile per-binding ceiling. Desktop reports 1-4 GiB; don't conflate the two paths.

## What's broken in the current allocations

`render/prepare/world.rs:320-346` derives the worst-case sizing for the 256×32×256 fixed world:

| Buffer | Current allocation | Mobile cap | Over by |
|---|---|---|---|
| `voxels` | 1024 MiB (`chunk_count × 128 × 4`) | 256 MiB | 4× |
| `blocks` | 512 MiB (`chunk_count × 64 × 4`) | 256 MiB | 2× |
| `taa_sample_accum` @ iPhone-like res | ~720 MiB (`pixels × 32 × 8`) | 256 MiB | ~2.8× |
| `taa_samples` @ same | ~720 MiB | 256 MiB | ~2.8× |
| `chunks` | 2.1 MiB (`chunk_count × 8`) | 256 MiB | fits |

Buffer-binding violation is a hard refuse on iOS Safari and a kernel-OOM on Mali (no graceful failure path either).

## The work — Task #7

Design and ship a **startup-time budget preselection routine** that picks safe sizes from `device.limits()` before any of the four oversized allocations happen.

### Three levers, in roughly decreasing order of cheapness

1. **TAA ring depth** — currently fixed at `DEFAULT_TAA_RING_DEPTH = 32` (`lib.rs:121`). Already plumbed as `AppArgs.taa_ring_depth` and a WGSL `#{TAA_SAMPLE_RING_DEPTH}` shader-def. Ladder: `{32, 24, 16, 8, 4, 0}` (0 = TAA disabled, supported in the existing `taa_ring_depth=0` lever). At iPhone-native ~3M pixels: 32 → ~720 MiB, 8 → ~180 MiB, 4 → ~90 MiB, 0 → 0. Pick the highest rung whose two buffers stay below the cap with headroom.

2. **Fixed-world chunk count** — currently `WORLD_SIZE_IN_SEGMENTS = (16, 2, 16)` in `world_size.rs:16`. C# canonical, but mobile profile cuts the XZ extent. Halving XZ → 128×32×128 chunks → blocks 128 MiB / voxels 256 MiB (voxels barely fits the cap). Quartering → 64×32×64 → blocks 32 MiB / voxels 64 MiB. The shader uses `voxelPos % modelSize` tiling so cutting XZ doesn't break content rendering; it caps where you can fly to. This is a "mobile profile" knob, not per-device — pick one for "mobile" once.

3. **Per-pixel buffer footprint** — `first_hit_data` (vec4<u32>) + `first_hit_absorption` + `final_color` at ~1170×2532 = 33 MiB + 16.5 MiB + 16.5 MiB ≈ 66 MiB. Within cap on its own, but rendering at 0.5× internal scale halves all four per-pixel buffers including TAA. Cheaper TAA budget AND fewer rays per frame — double win. Internal-resolution scale is its own lever.

### Concrete design questions for the next session

- Where does the budget routine live? Earliest plug-in point that can read `device.limits()` and *write* `AppArgs` / `WORLD_SIZE_IN_SEGMENTS` overrides before the world install fires. Probably a `Startup` system that runs `.before(setup_test_grid)` and inserts a `MobileBudget` resource the prepare-world step consults. `WORLD_SIZE_IN_SEGMENTS` is currently `pub const` — needs to migrate to a runtime resource (or a mobile-only override layer) without breaking desktop's compile-time uses.
- Headroom factor: spec says binding caps are HARD. Aim for ≤ 75% × cap per buffer (= 192 MiB) so resize-driven growth doesn't trip. Document the constant.
- Should buffer splitting (option 3 from the earlier mobile chat) be on the table, or strictly off-scope for this task? **Recommendation: off-scope.** Shader rewrite is weeks; the three levers above buy us mobile parity for a Mali-class device today.
- Probe-mode escape hatch: keep a `--probe` / env-var that bypasses `setup_test_grid` and dumps limits, so we can re-validate any new device cheaply. The current minimal-probe `android_main.rs` is essentially this — fold it into a flag on the real entry once budgets land.
- TAA-disable degradation: ring depth 0 disables temporal accumulation entirely. Visually the renderer loses one of the paper's load-bearing fidelity sources. Document this in the budget routine so we know the FPS-vs-noise tradeoff is explicit.

### Out of scope (do **not** start these here)

- Buffer splitting / multi-buffer routing in `voxels`/`blocks` shaders.
- iOS-specific build path. The cap fix is shared; iOS toolchain is a separate session.
- Touch input. Camera framing is fine for the FPS-gauge viability check the user wants.
- Stripped/optimised release builds. Strip debug then `dev` is sufficient; release-mode FPS only matters after budgets fit.

## What landed this session

- `crates/bevy_naadf/Cargo.toml`: `crate-type = ["rlib", "cdylib"]`, Android-targeted `bevy = { features = ["android-game-activity"] }`, `build = "build.rs"`.
- `crates/bevy_naadf/build.rs` (NEW): resolves `$ANDROID_NDK_HOME` etc. at build time and emits the link-arg for `libclang_rt.builtins-aarch64-android.a`. Portable; no host paths in tree. Read the file's doc-comment for the full rationale (outline-atomics from prebuilt libstd against `cargo-ndk` 4.1).
- `crates/bevy_naadf/src/lib.rs`: `#[cfg(target_os = "android")] pub mod android_main;`.
- `crates/bevy_naadf/src/android_main.rs` (NEW): currently the **minimal probe**, not the real entry. Doc-comment explains why. When budgets land, flip back to `build_app_with_args(AppConfig::windowed(), AppArgs::default())` after the budget routine has rewritten `AppArgs`.
- `.cargo/config.toml`: comment pointing at `build.rs` (no host paths).
- `android/` (NEW): full Gradle project. `app/build.gradle` includes an `applicationVariants.configureEach` block that wraps merged assets under `src/assets/` so Bevy's `AssetPlugin.file_path = "src/assets"` resolves on-device. `app/src/main/java/io/naadf/bevy/MainActivity.java` is the GameActivity bridge. `local.properties` is in tree but should be gitignored if it ever lands on a non-local checkout — it points at `/home/midori/Android/Sdk` (verify before commit; if it's tracked, gitignore + replace with `.example`).
- `.claude/worktrees/android-build` infrastructure for shipping APKs to a tethered device.

## Build commands that work

```bash
# 1. .so (~5 min cold, ~30s incremental, 1.9 GiB unstripped)
export ANDROID_NDK_HOME=/home/midori/Android/Sdk/ndk/28.2.13676358
export ANDROID_SDK_ROOT=/home/midori/Android/Sdk
cargo ndk -t arm64-v8a --platform 31 -o android/app/src/main/jniLibs build -p bevy-naadf --lib

# 2. strip-debug → ~190 MiB
"$ANDROID_NDK_HOME"/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-strip \
  --strip-debug android/app/src/main/jniLibs/arm64-v8a/libbevy_naadf.so

# 3. APK
export JAVA_HOME=/usr/lib/jvm/java-21-openjdk
export PATH="$JAVA_HOME/bin:$PATH"
export ANDROID_HOME=/home/midori/Android/Sdk
android/gradlew -p android assembleDebug

# 4. Install + launch
adb install -r -t android/app/build/outputs/apk/debug/app-debug.apk
adb logcat -c
adb shell am start -n io.naadf.bevy/.MainActivity
adb logcat | grep -E 'naadf-probe|RustStdoutStderr|FATAL|signal'
```

## Tripwires for the next session

- **Do not run the full-app build on a low-RAM device.** It WILL reboot the device on first launch via kernel OOM. Verify against device RAM before launching; if `MemTotal < 4 GiB`, run the probe path first.
- **Do not bump NDK version blindly.** `build.rs` globs `lib/clang/<major>/lib/linux/` and takes the first sub-dir. NDK r28 → clang 19. Future NDK r29 will likely ship clang 20 — the glob picks it up automatically, but verify by `llvm-nm -u` on the .so afterwards.
- **`local.properties` contains host paths.** If git-tracked, replace with a `.example` and gitignore.
- **`cargo build` from any binary (`bevy-naadf`, `e2e_render`, `bake`) on non-Android targets re-runs `build.rs`**, which exits in ~1 ms. No measurable cost, but worth knowing.
- **Mali-G52 driver is `r34p0`, mid-2022 vintage.** Older Mali drivers have known wgpu/Vulkan validation issues with storage buffers (atomic ops, indirect dispatch). If we ever fit the budget but render is still broken on Mali, suspect driver before shaders.
- **The 256 MiB cap is binding-stage, not per-buffer.** A 1 GiB buffer is legal as long as we only ever bind ≤ 256 MiB at a time. If the budget design decides to keep a single big underlying buffer with windowed bindings, that's spec-valid — call it out so the next reader doesn't assume "shrink the buffer."

## Side notes / observations

- `bevy_pbr::cluster: GPU clustering isn't supported on this device; falling back to CPU clustering.` and `bevy_render::batching::gpu_preprocessing: GPU preprocessing is not supported on this device. Falling back to CPU preprocessing.` show up on Mali. We don't use either path (no PBR scene), but it's a signal that Bevy's standard-pipeline features are quietly degrading on this hardware.
- `bevy_gilrs: Failed to start Gilrs. Gilrs does not support current platform.` — harmless on Android, gamepad plugin self-disables.
- Multiple `winit::platform_impl::android: TODO: ...` warnings (insets, content-rect, onStart forwarding) — winit's Android backend has gaps. Probably benign, but if windowing/orientation gets weird, suspect winit.
- Probe never reached the FPS gauge because there's no scene to render. When real budgets land and the world installs, add an FPS-to-logcat system so the viability decision has a number behind it, not vibes.
