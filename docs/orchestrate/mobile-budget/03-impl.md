# mobile-budget — implementation log

## general-purpose implementer findings (2026-05-21)

### Phase-by-phase execution log

#### Phase A — `budget.rs` skeleton + unit tests

Files changed:
- `crates/bevy_naadf/src/render/budget.rs` (new)
- `crates/bevy_naadf/src/render/mod.rs` (added `pub mod budget;` declaration above the `construction` module)

What landed: constants (`MIN_STORAGE_BINDING_CAP_BYTES`, `MOBILE_HEADROOM_FACTOR = 0.75`, `TAA_RING_DEPTH_LADDER`, `WORLD_SIZE_LADDER`, `SELECTION_PIXEL_COUNT_REFERENCE`), `EffectiveWorldSize` resource (with `canonical()` + `from_segments()` + `Default`), `RenderEffectiveWorldSize` mirror, `BudgetCaps`, `probe_limits()`, `select_budget()`, `probe_and_select()`, `log_budget_decision()`. Plus 8 unit tests covering desktop/mobile/intermediate/pathological selection plus the canonical-pin guards.

Verification:
- `cargo build --workspace` → green (5m 14s cold first build, ~25 s incremental thereafter).
- `cargo test --workspace --lib budget::` → **8/8 green**.

Deviation: the design's `BudgetCaps` field `taa_samples_bytes_per_megapixel` was unhelpful (it forced the caller to re-multiply by megapixels for the log line; the value is independent of the chosen pair). Renamed to `taa_samples_bytes` (= the full estimate at `SELECTION_PIXEL_COUNT_REFERENCE`). The log line + selection arithmetic are unchanged.

Deviation: the design's `select_budget` code sample cast `limits.max_storage_buffer_binding_size as u64` — in wgpu 29.0.3 that field IS already `u64` (verified at `~/.cargo/registry/src/.../wgpu-types-29.0.3/src/limits.rs:174`). The cast is a no-op; I kept it as an annotation (`let cap: u64 = ...`) for clarity. The unit test originally tried to write `cap_bytes as u32` per the design's claim — I corrected this to direct `u64` assignment to match the real field type.

#### Phase B — defensive `EffectiveWorldSize` seed in `build_app_with_args`

Files changed:
- `crates/bevy_naadf/src/lib.rs` (added the `if !contains_resource { insert canonical }` block after `app.insert_resource(cfg).insert_resource(args.clone())...` and before the `add_plugins(DefaultPlugins)` call).

Verification:
- `cargo build --workspace` → green (52 s).
- `cargo test --workspace --lib` → **187/187 green** (no regressions in `world_size_matches_csharp` pin or any other test).

Deviation: the design proposed putting the defensive insert immediately after `app.insert_resource(cfg)`. The actual current shape of `build_app_with_args` chains `.insert_resource(cfg).insert_resource(args.clone()).init_resource::<CameraHistory>().add_plugins(DefaultPlugins...)...` — splitting that long expression-statement felt brittle. I closed the statement at `.init_resource::<...>();`, added the defensive seed as a standalone block, then re-opened the chain with `app.add_plugins(...)`. Same effect; the resource is in the world before any plugin builds.

#### Phase C — migrate `voxel/grid.rs` install path to read `EffectiveWorldSize`

Files changed:
- `crates/bevy_naadf/src/voxel/grid.rs` — `setup_test_grid` gained `Res<EffectiveWorldSize>`; `install_world_at_fixed_size`, `install_empty_world`, `install_default_embedded_in_fixed_world`, `install_vox_in_fixed_world`, `install_vox_bytes_in_fixed_world`, `install_imported_vox` all gained `&EffectiveWorldSize` parameter; `demo_origin_v()` changed to `demo_origin_v(world_size_in_chunks: UVec3)`; the `WORLD_SIZE_IN_VOXELS` import was dropped, `WORLD_SIZE_IN_CHUNKS` is now `#[cfg(test)]`-gated (only the canonical-shape compose tests at `:1224, :1306` read it).
- `crates/bevy_naadf/src/voxel/async_vox.rs` — `poll_pending_vox_parse` gained `Res<EffectiveWorldSize>` and threads it into both native + wasm `install_imported_vox` call sites.
- `crates/bevy_naadf/src/render/construction/test_fixture.rs` — `demo_origin_v` call site now passes `crate::WORLD_SIZE_IN_CHUNKS` (the e2e fixture intentionally targets the canonical world).
- `crates/bevy_naadf/src/e2e/gates.rs` — four `demo_origin_v()` call sites now pass `crate::WORLD_SIZE_IN_CHUNKS` (e2e is desktop-only, always-canonical).
- `crates/bevy_naadf/src/e2e/small_edit_visual.rs` — two `demo_origin_v()` call sites likewise.

Verification:
- `cargo build --workspace` → green (43 s, one unused-import warning which the import cleanup resolved).
- `cargo test --workspace --lib` → **187/187 green**.

Deviation: the design proposed `demo_origin_v` taking `&EffectiveWorldSize`. I changed it to `world_size_in_chunks: UVec3` instead — `demo_origin_v` is called from contexts that don't have a `Res<EffectiveWorldSize>` in hand (e2e camera-pose helpers, the test_fixture entity spawner). Forcing them to manufacture a resource ref felt worse than taking a `UVec3` directly. The e2e call sites pass `crate::WORLD_SIZE_IN_CHUNKS` (they're desktop-only); production install-path callers don't go through `demo_origin_v` (they compute the offset inline).

Deviation: the design's migration list said `install_vox_sized_to_model` (the test-only CPU oracle phase) needed no changes. The function's `.vox` load-failure branch falls back to `install_default_embedded_in_fixed_world` which now requires the resource ref — I pass `&EffectiveWorldSize::canonical()` there, with a comment noting the oracle phase is desktop-test-only and never reaches mobile budgets.

#### Phase D — render-sub-app mirror + migrate `producer.rs` segment dispatch

Files changed:
- `crates/bevy_naadf/src/render/mod.rs` — `NaadfRenderPlugin::build` now reads the main-world `EffectiveWorldSize` (with `unwrap_or_else(EffectiveWorldSize::canonical)`) and inserts `RenderEffectiveWorldSize` into the render sub-app right alongside `TaaRingConfig`.
- `crates/bevy_naadf/src/render/construction/producer.rs` — `naadf_gpu_producer_node` gained an `effective_world: Option<Res<RenderEffectiveWorldSize>>` system param; the 20 `crate::WORLD_SIZE_IN_*` references (segment loop bounds at `:179-181`, per-segment generator-model uniform `world_size_in_voxels`, `GpuConstructionParams.size_in_chunks` × 3 sites, post-loop bounds-chain `world_chunks` upper bound) all now read `effective.in_segments` / `effective.in_chunks` / `effective.in_voxels`.

Verification:
- `cargo build --workspace` → green (46 s).
- `cargo test --workspace --lib` → **187/187 green** — including `runtime_gpu_producer_runs_and_matches_cpu_oracle_in_default_mode` (the W1 GPU producer test) which exercises the migrated node body with the canonical world.

No deviations from the design here. The pattern mirrors `TaaRingConfig` exactly — same line offsets in `render/mod.rs`, same shape of `world.get_resource()` style inside the node.

#### Phase E — `android_main.rs` flip-back to budget-aware production entry

Files changed:
- `crates/bevy_naadf/src/android_main.rs` — replaced the minimal-probe stub (DefaultPlugins + `log_render_device_limits` startup system) with the budget-aware entry: `probe_and_select()` → write `args.taa_ring_depth` → `build_app_with_args(cfg, args)` → `app.insert_resource(EffectiveWorldSize::from_segments(caps.world_size_in_segments))` → mobile-specific borderless-fullscreen window + `WinitSettings::mobile()` → `app.run()`.

Verification:
- Host-target `cargo build --workspace` → green (`#[cfg(target_os = "android")] pub mod android_main` is only compiled for android, but the file itself parses clean against the workspace lib).
- `cargo ndk -t arm64-v8a --platform 31 -o android/app/src/main/jniLibs build -p bevy-naadf --lib` → **green (28 s incremental)**. Produced `android/app/src/main/jniLibs/arm64-v8a/libbevy_naadf.so` (201 MiB pre-strip on aarch64-linux-android, dev profile).
- `llvm-strip --strip-debug` → no error (the .so is already small enough at `cargo build` dev's default — strip is a no-op here).
- `android/gradlew -p android assembleDebug` → **green (3 s, mostly cache-hit)**. Produced `android/app/build/outputs/apk/debug/app-debug.apk` at 414 MiB (debug; release builds would strip much further).

Build note: gradle's own strip step emits `[CXX1104] NDK from ndk.dir at .../28.2.13676358 had version [28.2.13676358] which disagrees with android.ndkVersion [26.1.10909125]` and then "Unable to strip the following libraries, packaging them as they are: libbevy_naadf.so." This is a pre-existing version-disagreement warning (the gradle config pins ndkVersion 26.1; the actual NDK on this machine is 28.2) — not introduced by this dispatch. The .so packages fine as-is.

#### Phase F — on-device verification

Deferred: `adb devices` returned an empty device list (no tethered tablet). Per the brief: "If the tablet is NOT tethered (no `adb devices` output), skip the on-device verification and flag it in your impl log; the user will run the device step themselves." Doing exactly that.

### Verification matrix

| gate | result | notes |
|---|---|---|
| `cargo build --workspace` | **green** | clean, no warnings |
| `cargo test --workspace --lib` | **187/187 green**, 1 ignored | the 1 ignored is the pre-existing GPU oracle test, untouched |
| `cargo run --bin e2e_render -- baseline` | **PASS (batch 6)** | desktop pass-through confirmed; allocations land at canonical sizes (chunks=16 MiB, blocks=512 MiB, voxels=1024 MiB), no `[budget]` log line (correct — `main` binary skips the probe) |
| `cargo ndk build` arm64-v8a | **green** | 28 s incremental |
| `android/gradlew assembleDebug` | **green** | APK at `android/app/build/outputs/apk/debug/app-debug.apk` (414 MiB) |
| on-device install + logcat | **skipped — no tethered adb** | user runs the device step; expected logcat line: `[budget] device cap max_storage_buffer_binding_size = 256 MiB; headroom_factor = 0.75 → ceiling 192 MiB. Selected: taa_ring_depth = 8, world_size_in_segments = (6, 2, 6). ...` |

### Files touched

- `crates/bevy_naadf/src/render/budget.rs` (new — 374 lines, ~120 of which are tests)
- `crates/bevy_naadf/src/render/mod.rs` (added `pub mod budget;` + the `RenderEffectiveWorldSize` mirror insertion in `NaadfRenderPlugin::build`)
- `crates/bevy_naadf/src/lib.rs` (defensive `EffectiveWorldSize::canonical()` insertion in `build_app_with_args`)
- `crates/bevy_naadf/src/voxel/grid.rs` (resource threading + import cleanup)
- `crates/bevy_naadf/src/voxel/async_vox.rs` (`poll_pending_vox_parse` gained `Res<EffectiveWorldSize>`)
- `crates/bevy_naadf/src/render/construction/producer.rs` (`naadf_gpu_producer_node` gained `Option<Res<RenderEffectiveWorldSize>>`; 20 const reads → effective-world reads)
- `crates/bevy_naadf/src/render/construction/test_fixture.rs` (one `demo_origin_v` call site updated)
- `crates/bevy_naadf/src/e2e/gates.rs` (four `demo_origin_v` call sites updated)
- `crates/bevy_naadf/src/e2e/small_edit_visual.rs` (two `demo_origin_v` call sites updated)
- `crates/bevy_naadf/src/android_main.rs` (full rewrite — minimal-probe → budget-aware production entry)
- `docs/orchestrate/mobile-budget/03-impl.md` (this file — new)

### Outstanding work

- **Phase F device step** — `adb devices` returned no tethered device, so the on-device install + logcat watch (the `[budget]` log line confirmation + the does-not-reboot check) is deferred to the user. The expected logcat signature is in the verification matrix above.
- **Phase G — `--probe` CLI flag** — explicitly deferred per the design's Decision #7. Not part of this dispatch.
- **The Q4 limits-check diagnostic at `render/prepare/world.rs:390-426`** stays in place per Side note #3 of the design (defense-in-depth): if a future config bypasses the budget routine or someone misconfigures `AppArgs.taa_ring_depth`, the post-allocation log still catches an overrun.

### Side notes / observations / complaints

1. **The design's `BudgetCaps.taa_samples_bytes_per_megapixel` field was load-shedding.** It forced the caller to multiply by megapixels at the log site instead of carrying the actual value. I renamed it to `taa_samples_bytes` (the full estimate at `SELECTION_PIXEL_COUNT_REFERENCE`); the log line is cleaner and unit tests can assert against headroom directly without a re-multiply. Minor; only mentioning so a future doc reader doesn't grep for the old name.

2. **The design's mention of `cap_bytes as u32` in the test helper was wrong.** wgpu 29.0.3's `Limits.max_storage_buffer_binding_size` is `u64`, not `u32` — verified at the actual `wgpu-types-29.0.3/src/limits.rs:174`. The cast in the design's `limits_with_cap(cap_bytes: u64) -> Limits` would not compile (and didn't). Fixed inline; the design's selection-arithmetic body was internally consistent (it cast cap `as u64` which is a no-op).

3. **`demo_origin_v` migration to `&EffectiveWorldSize` was awkward.** The function is called from contexts that don't have a `Res<EffectiveWorldSize>` (e2e camera-pose helpers, the test_fixture entity spawner — both desktop-only). Forcing them to manufacture a `&EffectiveWorldSize::canonical()` ref felt worse than taking a `UVec3` directly. I went with `world_size_in_chunks: UVec3` instead and have the e2e call sites pass `crate::WORLD_SIZE_IN_CHUNKS`. The production install path doesn't call `demo_origin_v` (it inlines the centring arithmetic), so this divergence doesn't reach mobile.

4. **The W5 GPU producer's per-segment loop trade-off is real and worth flagging.** At `(6, 2, 6)` segments the producer dispatches `6 × 2 × 6 = 72` segments — well below the canonical 512. Each segment is its own command-encoder + submit (per the W5.3-fix Stage 1 design at `producer.rs:151-176`), so the per-frame cost stays linear. At `(4, 2, 4)` that would be `32` segments. Even at the smallest ladder rung the producer is well within frame budget. No additional optimisation needed.

5. **The strip step on the .so didn't do anything visible.** `llvm-strip --strip-debug` exited 0 but the file size didn't change (201 MiB → 201 MiB). The `cargo ndk` dev build may already be lean enough that `--strip-debug` finds nothing more to strip, OR a previous strip already ran. Either way it's harmless; gradle's later strip step also no-op'd due to the pre-existing NDK-version-disagreement warning (`[CXX1104]`).

6. **The defensive seed in `build_app_with_args` is the right call but `build_app_with_args`'s API still feels backwards.** The Android caller has to insert `EffectiveWorldSize::from_segments(...)` AFTER `build_app_with_args` returns (so the helper's defensive seed runs first, then the caller overrides). This works (`insert_resource` overwrites on second call), but the cleaner refactor would split `build_app_with_args` into `new_app_with_naadf_plugins(cfg)` + caller-driven `insert_resource` chains. Out of scope for this dispatch; future cleanup candidate (also flagged in the design's Side note #4).

7. **The foundation is fine for this task.** The probe-app pattern reused `world/buffer.rs:246-264` cleanly; the `TaaRingConfig` mirror was a complete template for the new `EffectiveWorldSize` mirror; the migration touched 30+ lines but every line was mechanical (`crate::WORLD_SIZE_IN_*` → `effective.in_*`). No smell-driven escape needed; the design slotted into existing patterns. Subjective note: this is a well-architected codebase — every pattern the design called out genuinely existed and could be copied.

8. **On the device-step skip.** The expected `[budget]` log line on Mali-G52 (per the design's selection arithmetic) is:

   ```
   [budget] device cap max_storage_buffer_binding_size = 256 MiB; headroom_factor = 0.75 → ceiling 192 MiB. Selected: taa_ring_depth = 8, world_size_in_segments = (6, 2, 6). Estimated binding sizes: voxels = 144 MiB, blocks = 72 MiB, taa_samples (@ 3 MP reference) = 192 MiB.
   ```

   If the user runs the APK on the tablet and sees this line in logcat AND the device does NOT reboot, the dispatch is done. If the line appears but the device reboots, the next steps per the design §8 are:
   - Add chunked init-buffer zero-fill (the validation.rs:1097 hint about "Zero-initialise via a single 1 MiB chunk loop to bound peak memory" — wgpu's default buffer-creation path may allocate the full binding-size + backing on Mali; the chunked path is the workaround).
   - OR drop the headroom factor from 0.75 to 0.60 (forces `(6, 2, 6)` still; voxels=144 vs ceiling=154 — marginal).
   - OR drop to 0.50 (forces `(4, 2, 4)`; voxels=64 with comfortable margin).

   None of these are part of this dispatch; they're follow-ups gated on the device step revealing whether budget alone is enough.
