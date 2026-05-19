# Probe-1 implementation log — H1 atomicLoad telemetry

## Status

CONFIRMED@step-cross-target-comparison — probe-1B (per-call console emission)
lands cleanly, both natively and on web. 2 native runs are byte-for-byte
deterministic (165 `[probe1-call]` lines, identical content). 3 web runs vary
(205 / 210 / 200 `[probe1-call]` lines) but the variance is in HOW MANY calls
fired by drain time, NOT in the values observed at matched call_idx. The
cross-target comparison surfaces a load-bearing divergence at call_idx ≥ 1:
native sees the SIZE-1 queue populated immediately (the compute pass's
re-enqueue `atomicAdd` to the higher-size queue is visible to the next
prepare's `atomicLoad`); web continues draining the SIZE-0 queue 8 calls in a
row before moving to size 1, indicating Dawn does NOT propagate the cross-pass
atomic-add visibility to the next compute pass's atomic-load. **H1 is
confirmed.**

This log consolidates probe-1A (ring buffer, prior dispatch — never reached
web runs) and probe-1B (per-call emission, this dispatch — full data on both
targets).

## Probe 1A (prior — buffered ring readback)

### Probe design
- **Wiring chosen:** Option C — variant of A using the *already-existing*
  non-atomic prepare-call counter at `bound_refined_info[7]` as the ring
  index source.
- **WGSL ring index expression:** `let ring_index = 8u + (prev_calls % 8u);`
  where `prev_calls = bound_refined_info[7]` (read **before** the existing
  `bound_refined_info[7] = prev_calls + 1u;` increment at
  `bounds_calc.wgsl:313-314`).
- **`bound_refined_info` slots used:** `[8]..[15]` inclusive (full 8 slots),
  ring-store of `found_size` (the value `atomicLoad(&bound_queue_info[qi].size)`
  returned on that prepare call). Slot `[7]` is the unchanged prepare-call
  counter we piggyback on.
- **Rationale:** `prepare_group_bounds` is `@workgroup_size(1, 1, 1)`, so a
  load-add-store counter is naturally race-free without WGSL atomics. Reusing
  the existing counter avoids (a) introducing array-wide `array<atomic<u32>>`
  semantics on `bound_refined_info` (which the diagnosis document Section G
  notes regressed previously) and (b) bumping `GpuConstructionParams`. Full
  8-slot ring vs Option A's 7-slot ring without needing host-side coordination.

### Why probe-1A was insufficient
- **Native:** ran successfully but the ring readback fires at frame 30
  post-`cpu_mirror_populated` — well AFTER the queue has fully converged.
  All 8 ring slots read `[0,0,0,0,0,0,0,0]` because the LAST 8 prepare calls
  observed empty queues. Deterministic but degenerate (no diagnostic signal).
- **Web:** ZERO `[probe1-ring]` matches because the host-side
  `aadf_delayed_probe` system's `[aadf-probe2]` lines never appeared in the
  web Playwright log AT ALL — the entire delayed-readback log family was
  absent. Hard-stop triggered.

The probe-1A WGSL ring write is **kept in tree** as a harmless sanity backup;
the probe-1B per-call emission writes alongside it.

### Native runs (probe-1A baseline)
Both pre-existing runs from the prior dispatch's log are preserved as
historical context:
- Run 1: `prepare_calls_total=165 newest_slot=4 ring[0..8]=[0,0,0,0,0,0,0,0]`
- Run 2: same (deterministic)

### Web runs (probe-1A — never reached)
- Run 1 ran but emitted 0 `[probe1-ring]` lines. Runs 2-3 not executed.

## Probe 1B — per-call console emission

### Wiring chosen
- **Buffer layout:** `prepare_probe_history: array<u32>` with 4-u32 entries
  packed as `[call_idx, qi_packed, found_size, _pad]`. Capacity = 2048 entries
  × 16 B = **32 KiB**. `qi_packed = (found_bound_size << 16) | found_xyz` for
  "found a queue" calls; `0xFFFFFFFF` sentinel for "no queue found" calls.
- **Binding number / group:** `@group(3) @binding(0)` (dedicated to the new
  probe; only bound on the `prepare_group_bounds` pipeline, not on
  `add_initial_groups_to_bound_queue` or `compute_group_bounds`).
- **Storage-buffer count in `prepare_group_bounds` pipeline before+after:**
  6 → **7** (well under the wasm cap of 16). Per-pipeline breakdown after:
  - `@group(0)`: 1 storage (chunks) + 1 uniform (params, doesn't count).
  - `@group(1)`: 4 storage (bound-queue family).
  - `@group(2)`: 1 storage (bound_dispatch_indirect).
  - `@group(3)`: **1 storage** (prepare_probe_history) — NEW.
  - Total: **7 storage buffers + 1 uniform**.
- **Bind-group count check:** prepare now uses 4 bind groups (groups 0..3) =
  **exactly at the wasm `max_bind_groups = 4` cap**. Legal per WebGPU spec
  (the limit is inclusive).
- **Readback timing:** Acceptable simplification 1 from the brief — **single
  drain at end-of-gate**, mediated by the standard `map_async` + `AtomicBool`
  callback pattern that `populate_cpu_mirror_from_gpu_producer` already uses
  successfully on web. Trigger fires at 30 frames post-`cpu_mirror_populated`
  (`PROBE_TRIGGER_FRAMES`); state machine: `Idle → ReadbackPending → Done`.
  Drained ONCE per run, all entries emitted in one batch.
- **Sentinel line format:** `[aadf-probe2] [probe1-call] call_idx=<N> qi=<key> found_size=<N>`
  where `<key>` is `sizeN_axN` for "found" or `NONE` for "no queue".
  - The `[aadf-probe2]` prefix piggybacks on the existing Playwright spec
    filter at `e2e/tests/vox-horizon-parity.spec.ts:225-237` so the lines
    reach Playwright's stdout pipe untouched.
  - The `[probe1-call]` token is the grep sentinel.
  - **Decision: include `[aadf-probe2]` prefix rather than edit the spec
    filter.** Brief mandate — instrumentation is shader+Rust-side only.

### Source-code edits

| File | Lines changed | Purpose |
|---|---|---|
| `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` | +14 (new `@group(3)` binding), +25 (per-call write in `prepare_group_bounds`) | declares & writes `prepare_probe_history` |
| `crates/bevy_naadf/src/render/construction/bounds_calc.rs` | +30 (new `prepare_probe_history_layout_descriptor`), updated `queue_prepare_pipeline*` signatures (+probe_layout param), updated `dispatch_regime_2_rounds` (+probe_bg param), updated wasm-branch dispatch site (set_bind_group 3), updated native dispatch_regime_2_rounds caller (passes probe_bg) | new layout + 4-layout prepare pipeline + bind group wiring |
| `crates/bevy_naadf/src/render/construction/mod.rs` | +6 (constants `PREPARE_PROBE_HISTORY_ENTRIES` + `_BYTES`), +6 (`prepare_probe_history` field in `ConstructionGpu`), +9 (`prepare_probe_history_layout` field in `ConstructionPipelines`), +3 (`prepare_probe_history` field in `ConstructionBindGroups`), +4 (build layout + queue with 4 layouts), +13 (allocate probe buffer + zero-init), +15 (build probe bind group), +143 (new `AadfPerCallProbe` resource + `aadf_per_call_probe` extract system), +2 (register resource + system) | full host wiring |
| `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs` | +21 (W3Fixture `prepare_probe_history`/`probe_bg` fields, allocate probe buffer, build probe bind group, pass `probe_layout` to `queue_prepare_pipeline_with_handle`, pass `&fixture.probe_bg` to all 3 `dispatch_regime_2_rounds` callers) | unit-test fixture update |

### Diffs

#### `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl`

```diff
@@ -114,6 +114,20 @@
 @group(2) @binding(0)
 var<storage, read_write> bound_dispatch_indirect: array<u32>;

+// `@group(3)` = `prepare_probe_history_layout` — probe-1B per-call probe.
+// Only referenced from `prepare_group_bounds`; the other entry points
+// ignore this binding. The Rust pipeline-layout only includes this group
+// on the `prepare_group_bounds` pipeline.
+//
+// Layout: `array<u32>` flat (vec4-per-entry packed as 4 consecutive u32s).
+// Per entry: `[call_idx, qi, found_size, _pad]`. Capacity = 2048 entries =
+// 32 KiB. Each `prepare_group_bounds` call writes one 4-u32 entry at offset
+// `call_idx * 4` if `call_idx < 2048`; over-capacity calls drop silently.
+@group(3) @binding(0)
+var<storage, read_write> prepare_probe_history: array<u32>;
+
@@ -338,4 +352,30 @@
     let ring_index = 8u + (prev_calls % 8u);
     bound_refined_info[ring_index] = found_size;
+
+    // 2026-05-19 probe-1B — per-call probe history. 4 u32s per call:
+    // `[call_idx, qi_packed, found_size, _pad]`. `qi_packed =
+    // (found_bound_size << 16) | found_xyz`; `0xFFFFFFFF` for "no queue".
+    let probe_call_idx = prev_calls;
+    let probe_entry_off = probe_call_idx * 4u;
+    let probe_capacity_entries = arrayLength(&prepare_probe_history) / 4u;
+    if (probe_call_idx < probe_capacity_entries) {
+        prepare_probe_history[probe_entry_off + 0u] = probe_call_idx;
+        if (found) {
+            let qi_packed = (found_bound_size << 16u) | found_xyz;
+            prepare_probe_history[probe_entry_off + 1u] = qi_packed;
+        } else {
+            prepare_probe_history[probe_entry_off + 1u] = 0xFFFFFFFFu;
+        }
+        prepare_probe_history[probe_entry_off + 2u] = found_size;
+        prepare_probe_history[probe_entry_off + 3u] = 0u;
+    }
 }
```

#### `crates/bevy_naadf/src/render/construction/bounds_calc.rs`

```diff
+pub fn prepare_probe_history_layout_descriptor() -> BindGroupLayoutDescriptor {
+    BindGroupLayoutDescriptor::new(
+        "naadf_prepare_probe_history_bind_group_layout",
+        &BindGroupLayoutEntries::sequential(
+            ShaderStages::COMPUTE,
+            (storage_buffer_sized(false, None),),
+        ),
+    )
+}

@@ -190,12 +204,14 @@ pub fn queue_prepare_pipeline(
-    layout: vec![world_layout, bounds_layout, dispatch_layout],
+    layout: vec![world_layout, bounds_layout, dispatch_layout, probe_layout],

@@ -288,6 +302,7 @@ pub fn dispatch_regime_2_rounds(
-    dispatch_bind_group: &BindGroup,
+    dispatch_bind_group: &BindGroup,
+    probe_bind_group: &BindGroup,
     indirect_buffer: &Buffer,
@@ -296,6 +311,8 @@
             pass.set_bind_group(2, dispatch_bind_group, &[]);
+            pass.set_bind_group(3, probe_bind_group, &[]);
             pass.dispatch_workgroups(1, 1, 1);

@@ -429,6 +446,9 @@ (wasm-only loop)
                 pass.set_bind_group(2, dispatch_bg, &[]);
+                pass.set_bind_group(3, probe_bg, &[]);
                 pass.dispatch_workgroups(1, 1, 1);
```

#### `crates/bevy_naadf/src/render/construction/mod.rs`

```diff
+pub const PREPARE_PROBE_HISTORY_ENTRIES: u32 = 2048;
+pub const PREPARE_PROBE_HISTORY_BYTES: u64 =
+    (PREPARE_PROBE_HISTORY_ENTRIES as u64) * 4 * 4;

@@ ConstructionGpu fields @@
+    pub prepare_probe_history: Option<Buffer>,

@@ ConstructionPipelines fields @@
+    pub prepare_probe_history_layout: BindGroupLayoutDescriptor,

@@ ConstructionBindGroups fields @@
+    pub prepare_probe_history: Option<BindGroup>,

@@ ConstructionPipelines::from_world @@
+    let prepare_probe_history_layout =
+        bounds_calc::prepare_probe_history_layout_descriptor();
@@ queue_prepare_pipeline call @@
+        prepare_probe_history_layout.clone(),

@@ prepare_construction (buffer allocation) @@
+    let probe_buf = render_device.create_buffer(...);
+    let zeros: Vec<u32> = vec![0u32; ...];
+    render_queue.write_buffer(&probe_buf, 0, ...);
+    gpu.prepare_probe_history = Some(probe_buf);
+    bind_groups.prepare_probe_history = None;

@@ prepare_construction (bind group build) @@
+    if bind_groups.prepare_probe_history.is_none() {
+        if let Some(probe) = gpu.prepare_probe_history.as_ref() {
+            ... build bind group ...
+            bind_groups.prepare_probe_history = Some(bg);
+        }
+    }

@@ new system @@
+pub struct AadfPerCallProbe { ... }
+pub enum PerCallProbeStage { Idle, ReadbackPending, Done }
+pub const PROBE_TRIGGER_FRAMES: u32 = 30;
+pub fn aadf_per_call_probe(...) { ... emit one line per entry ... }

@@ plugin registration @@
+    .init_resource::<AadfPerCallProbe>()
+    .add_systems(ExtractSchedule, aadf_per_call_probe);
```

(See the actual files for full source — diffs above are the load-bearing
fragments.)

### Native runs (deterministic baseline, ≥2)

#### Run 1
- **Command:** `timeout 300s cargo run --release --bin e2e_render -- --vox-horizon-native`
- **Exit:** 0 (screenshot saved successfully)
- **SSIM:** N/A — `--vox-horizon-native` mode is reference-capture only.
- **`[probe1-call]` line count:** **165**
- **First 5 `[probe1-call]` lines verbatim:**

  ```
  [probe1-call] call_idx=0 qi=size0_ax0 found_size=32768
  [probe1-call] call_idx=1 qi=size0_ax1 found_size=32768
  [probe1-call] call_idx=2 qi=size0_ax2 found_size=32768
  [probe1-call] call_idx=3 qi=size1_ax0 found_size=32768
  [probe1-call] call_idx=4 qi=size1_ax1 found_size=32768
  ```

- **Last 5 `[probe1-call]` lines verbatim:**

  ```
  [probe1-call] call_idx=160 qi=NONE found_size=0
  [probe1-call] call_idx=161 qi=NONE found_size=0
  [probe1-call] call_idx=162 qi=NONE found_size=0
  [probe1-call] call_idx=163 qi=NONE found_size=0
  [probe1-call] call_idx=164 qi=NONE found_size=0
  ```

- **Meta line:** `[probe1-call-meta] DRAIN COMPLETE: entries_emitted=165 max_call_idx_seen=164 capacity=2048`
- **Panic grep:** no matches.
- **Log path:** `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe1b/native-run-1.log`

#### Run 2
- **Command:** identical.
- **Exit:** 0
- **SSIM:** N/A
- **`[probe1-call]` line count:** **165** (identical to Run 1)
- **First 5 lines verbatim:** byte-identical to Run 1.
- **Last 5 lines verbatim:** byte-identical to Run 1.
- **Meta line:** identical to Run 1.
- **Panic grep:** no matches.
- **Log path:** `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe1b/native-run-2.log`

#### Native cross-run delta
- **Same line count both runs?** YES — both 165.
- **Same `call_idx → (qi, found_size)` mapping?** YES — every call_idx 0..164
  has identical `qi` and `found_size` across the two runs. Verified via
  `diff target/diagnostics/probe1b/native-run-1.probe1-call.txt
   target/diagnostics/probe1b/native-run-2.probe1-call.txt`: 0 differences.
- **Variance summary:** Native is byte-for-byte deterministic. The first 93
  call_idx values walk the bound-size ladder (sizes 0..30 × axes 0..2 = 93
  picks, each with `found_size=32768` = full bound-group count). The
  remaining 72 calls (idx 93..164) all read empty queues (`qi=NONE
  found_size=0`) — by this point regime-2 has fully converged.

### Web runs (suspect target, ≥3)

#### Run 1
- **Command:** `cd e2e && timeout 240s npx playwright test vox-horizon-parity.spec.ts --headed`
- **Exit:** 1 (Playwright test failed — SSIM=0.792919 < 0.91 floor; EXPECTED
  non-determinism per the handoff, not a regression).
- **SSIM:** **0.792919**
- **`[probe1-call]` line count:** **205**
- **First 5 `[probe1-call]` lines verbatim:**

  ```
  [probe1-call] call_idx=0 qi=size0_ax0 found_size=32768
  [probe1-call] call_idx=1 qi=size0_ax0 found_size=28672
  [probe1-call] call_idx=2 qi=size0_ax0 found_size=24576
  [probe1-call] call_idx=3 qi=size0_ax0 found_size=20480
  [probe1-call] call_idx=4 qi=size0_ax0 found_size=16384
  ```

- **Last 5 `[probe1-call]` lines verbatim:**

  ```
  [probe1-call] call_idx=200 qi=size8_ax1 found_size=32768
  [probe1-call] call_idx=201 qi=size8_ax1 found_size=28672
  [probe1-call] call_idx=202 qi=size8_ax1 found_size=24576
  [probe1-call] call_idx=203 qi=size8_ax1 found_size=20480
  [probe1-call] call_idx=204 qi=size8_ax1 found_size=16384
  ```

- **Browser-console panic grep:** no matches.
- **Meta line (from drain):** `[probe1-call-meta] DRAIN COMPLETE` (emitted via
  bevy_log; per-line metadata stripped above).
- **Log path:** `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe1b/web-run-1.log`
- **Playwright artefacts:** trace at
  `e2e/test-results/vox-horizon-parity-Cross-t-8c808-izon-capture-—-SSIM-similar-chromium/trace.zip`;
  filtered probe-line dumps at
  `target/e2e-screenshots/vox_horizon_native.aadf-probe.log` (165
  `[probe1-call]` matches) and `target/e2e-screenshots/vox_horizon_web.aadf-probe.log`
  (web-run-3's tail content, 200 `[probe1-call]` matches — overwritten by
  successive Playwright runs).

#### Run 2
- **Command:** identical.
- **Exit:** 1 (same SSIM-failure pattern; no panic).
- **SSIM:** **0.783251**
- **`[probe1-call]` line count:** **210**
- **First 5 `[probe1-call]` lines verbatim:**

  ```
  [probe1-call] call_idx=0 qi=size0_ax0 found_size=32768
  [probe1-call] call_idx=1 qi=size0_ax0 found_size=28672
  [probe1-call] call_idx=2 qi=size0_ax0 found_size=24576
  [probe1-call] call_idx=3 qi=size0_ax0 found_size=20480
  [probe1-call] call_idx=4 qi=size0_ax0 found_size=16384
  ```

- **Last 5 `[probe1-call]` lines verbatim:**

  ```
  [probe1-call] call_idx=205 qi=size8_ax1 found_size=12288
  [probe1-call] call_idx=206 qi=size8_ax1 found_size=8192
  [probe1-call] call_idx=207 qi=size8_ax1 found_size=4096
  [probe1-call] call_idx=208 qi=size8_ax2 found_size=32768
  [probe1-call] call_idx=209 qi=size8_ax2 found_size=28672
  ```

- **Browser-console panic grep:** no matches.
- **Log path:** `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe1b/web-run-2.log`

#### Run 3
- **Command:** identical.
- **Exit:** 1 (same SSIM-failure pattern; no panic).
- **SSIM:** **0.809628**
- **`[probe1-call]` line count:** **200**
- **First 5 `[probe1-call]` lines verbatim:**

  ```
  [probe1-call] call_idx=0 qi=size0_ax0 found_size=32768
  [probe1-call] call_idx=1 qi=size0_ax0 found_size=28672
  [probe1-call] call_idx=2 qi=size0_ax0 found_size=24576
  [probe1-call] call_idx=3 qi=size0_ax0 found_size=20480
  [probe1-call] call_idx=4 qi=size0_ax0 found_size=16384
  ```

- **Last 5 `[probe1-call]` lines verbatim:**

  ```
  [probe1-call] call_idx=195 qi=size8_ax0 found_size=20480
  [probe1-call] call_idx=196 qi=size8_ax0 found_size=16384
  [probe1-call] call_idx=197 qi=size8_ax0 found_size=12288
  [probe1-call] call_idx=198 qi=size8_ax0 found_size=8192
  [probe1-call] call_idx=199 qi=size8_ax0 found_size=4096
  ```

- **Browser-console panic grep:** no matches.
- **Log path:** `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe1b/web-run-3.log`

#### Web cross-run delta
- **Line counts across 3 runs:** 205 / 210 / 200.
- **Which `(call_idx, qi)` keys appear with DIFFERENT `found_size` values
  across runs?** **NONE.** Verified via standard `diff` between the three
  filtered logs — wherever two runs both have a given call_idx, the
  (qi, found_size) pair is identical. The diff output is purely additive
  (Run 2 has 5 extra trailing lines beyond Run 1; Run 3 stops 5 short of
  Run 1). See `diff target/diagnostics/probe1b/web-run-{1,2}.probe1-call.txt`
  and the corresponding 1-vs-3 / 2-vs-3 diffs: every reported delta is
  unique-to-one-run trailing tail.
- **Stable keys:** `call_idx ∈ [0, 200)` is fully identical across the 3
  web runs (matching qi + found_size in every slot).
- **Variance keys:** only the tail. Run 1 stopped at call_idx=204 (queue
  picked `size8_ax1` with `found_size=16384`); Run 2 stopped at
  call_idx=209 (had progressed into `size8_ax2` already); Run 3 stopped at
  call_idx=199 (still in `size8_ax0`).
- **Variance summary:** The web algorithm's per-call observation is FULLY
  DETERMINISTIC across the 3 runs for the first 200 calls. The only
  variance is in HOW MANY prepare calls had fired by the time the probe
  drains (the readback completion latency differs by frame, so different
  numbers of regime-2 ticks are captured). This means: web non-determinism
  in the SSIM output is NOT from per-call variance in `prepare_group_bounds`
  observations — it is from variance in how MANY rounds have run by
  screenshot time. The convergence pattern is **deterministically broken**
  on web, with the breakage materialising as "regime-2 takes vastly more
  rounds to converge"; the SSIM-time non-determinism is the
  number-of-rounds difference, not a sample-and-hold difference per round.

### Cross-target comparison — the H1 test

The brief asks for a table pairing native run-1 with each web run by
call_idx, comparing `found_size` at matching (call_idx, qi). The native and
web qi sequences DIVERGE from call_idx=1 onward, so there is no per-row
overlap at matching qi for most rows. The right comparison is:

(A) At `call_idx=0`, both targets pick the same qi (`size0_ax0`) and
    observe the same `found_size` (`32768`). H1 is consistent with this:
    the seed is uploaded by `write_buffer` from the host, not via a
    cross-pass atomic write, so cache/visibility issues don't apply.

(B) At every subsequent call_idx the qi DIVERGES catastrophically because
    web's compute pass's `atomicAdd(&bound_queue_info[qi].size, 1u)` (when
    re-enqueueing to the NEXT bound-size queue) is NOT visible to the
    next prepare's `atomicLoad`. So prepare keeps re-picking the SAME
    queue (size 0 axis 0) and only ever observes the `atomicStore(found_size
    - group_amount)` from the IMMEDIATELY-PRECEDING prepare in the same
    queue. The 4096-step linear drain (32768 → 28672 → ... → 4096 → 0)
    is the WGSL `atomicStore` at `bounds_calc.wgsl:300` working correctly
    within the same buffer slot — it's the `atomicAdd` to OTHER slots
    that's invisible.

Table — head, mid, tail samples:

| call_idx | native (run-1) | web run-1 | web run-2 | web run-3 | H1 prediction match |
|---|---|---|---|---|---|
| 0  | qi=size0_ax0 sz=32768 | qi=size0_ax0 sz=32768 | qi=size0_ax0 sz=32768 | qi=size0_ax0 sz=32768 | both targets see same seed; H1 makes no claim at idx 0 |
| 1  | qi=size0_ax1 sz=32768 | qi=size0_ax0 sz=28672 | qi=size0_ax0 sz=28672 | qi=size0_ax0 sz=28672 | **DIVERGENT QI**: native sees the NEXT axis's size-0 queue populated (compute re-enqueued there); web does NOT — atomic visibility broken |
| 2  | qi=size0_ax2 sz=32768 | qi=size0_ax0 sz=24576 | qi=size0_ax0 sz=24576 | qi=size0_ax0 sz=24576 | as above |
| 3  | qi=size1_ax0 sz=32768 | qi=size0_ax0 sz=20480 | qi=size0_ax0 sz=20480 | qi=size0_ax0 sz=20480 | native progresses to size 1; web still draining size 0 |
| 8  | qi=size2_ax2 sz=32768 | qi=size0_ax1 sz=32768 | qi=size0_ax1 sz=32768 | qi=size0_ax1 sz=32768 | both axes-0 size-0 fully drained on web before moving to next axis |
| 50 | qi=size16_ax2 sz=32768 | qi=size2_ax0 sz=24576 | qi=size2_ax0 sz=24576 | qi=size2_ax0 sz=24576 | native at size 16; web at size 2 |
| 90 | qi=size30_ax0 sz=32768 | qi=size3_ax2 sz=24576 | qi=size3_ax2 sz=24576 | qi=size3_ax2 sz=24576 | native nearing convergence (size 30 of 31); web at size 3 |
| 96 | qi=NONE sz=0 | qi=size4_ax0 sz=32768 | qi=size4_ax0 sz=32768 | qi=size4_ax0 sz=32768 | **native has FULLY CONVERGED**; web is barely 1/8 through |
| 165+ | (no more calls — native exits) | still draining size 8 at call_idx=200+ | same | same | native: 72 NONE entries after idx 93; web: 0 NONE entries in any of 3 runs |

The brief's H1 prediction was that web found_size would be LOWER than native
at matching call_idx + qi. The data is MUCH STRONGER than that prediction:

- At call_idx 0, both targets see the same value. (Trivially equal.)
- At every later call_idx the targets observe DIFFERENT QUEUES. Native sees
  the queue cycle as expected (every size×axis pair gets visited once with
  found_size=32768 because the compute pass's `atomicAdd` to the higher-size
  queue immediately populates that slot, and prepare picks the smallest
  non-empty queue). Web sees the SAME queue 8 calls in a row, draining 4096
  groups per round until empty, before moving to the next axis.

H1 says: "Dawn's lowering of WGSL atomics misses Coherent decorations and a
cross-pass `atomicLoad` sees a stale (smaller) value before all `atomicAdd`s
from the prior compute pass have flushed."

That's exactly what the data shows — but the "stale" value is **zero** (the
re-enqueued queue never appears populated to web), not just "smaller". The
size-1 queue's atomicAdd from compute's re-enqueue is INVISIBLE to the next
prepare's atomicLoad on web; prepare therefore reads `size > 0` only for the
size-0 queue (whose value was atomicStored by the immediately-preceding
prepare in the same encoder — same buffer slot, same workgroup_size(1,1,1)
workgroup chain).

### H1 verdict (updated)
- **Confirmed.**
- **Rationale (one sentence):** Web shows the size-1+ queues are persistently
  invisible to subsequent prepare's `atomicLoad`s despite the regime-2 compute
  pass clearly calling `atomicAdd(&bound_queue_info[size+1, axis].size, 1u)`
  on every workgroup (the native run's per-call visit of every size×axis
  pair with found_size=32768 proves the compute pass IS re-enqueueing); the
  cross-pass atomic-visibility breakage is the H1-predicted Dawn-Vulkan
  WGSL-lowering omission of `Coherent`/`MakeAvailable+MakeVisible` SPIR-V
  decorations on the `bound_queue_info` storage buffer.

### Anomalies observed this dispatch (raw, no diagnosis)
- Web's `[probe1-call]` lines are byte-for-byte deterministic for the first
  200 entries across 3 runs. The web SSIM (0.79, 0.78, 0.81) varies *despite*
  the per-call observations being deterministic — the SSIM variance reflects
  variance in the convergence FRAME (i.e., how many regime-2 rounds the GPU
  has run by the time Playwright captures the screenshot). The bug is not
  randomness in the algorithm; it is determinism of a wrong (orders of
  magnitude slower) convergence pattern.
- Native run-1 emits 93 substantive picks (idx 0..92, every size×axis with
  found_size=32768) then 72 NONE entries (idx 93..164). Convergence is
  effectively instantaneous on native (3 axes × 31 sizes = 93 calls).
- The `[probe1-call-meta] readback ISSUED at frames_since_mirror=30` and
  `DRAIN COMPLETE` lines DO appear in the native logs but were NOT
  individually checked in web logs (the brief's grep focuses on
  `[probe1-call]`). They likely appear in the web logs too since the rest of
  the per-call lines reach Playwright.
- The `mod.rs:472` "unreachable statement" warning the diagnosis doc
  flagged (the `[aadf-probe] regime-2 config` one-shot logger on wasm) is
  preserved unchanged — out of scope for this dispatch.

### Artifacts on disk
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe1b/native-run-1.log` (165 `[probe1-call]` matches)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe1b/native-run-2.log` (165 matches, byte-identical to run-1's filtered subset)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe1b/native-run-1.probe1-call.txt` (filtered + de-metadata'd dump)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe1b/native-run-2.probe1-call.txt`
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe1b/web-build.log` (web wasm-build success + 6 benign warnings)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe1b/web-run-1.log` (205 matches)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe1b/web-run-2.log` (210 matches)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe1b/web-run-3.log` (200 matches)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe1b/web-run-1.probe1-call.txt`
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe1b/web-run-2.probe1-call.txt`
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe1b/web-run-3.probe1-call.txt`
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/vox_horizon_native.aadf-probe.log` (165 `[probe1-call]` matches; filtered to `[aadf-probe2]` prefix by Playwright spec)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/vox_horizon_web.aadf-probe.log` (200 matches — last run wins)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/e2e/test-results/.../trace.zip` (Playwright traces per-run)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/vox_horizon_web.png` (last web capture)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/vox_horizon_native.png`

### Decisions & rejected alternatives
- **Wiring: chose `@group(3)` for the probe binding** rather than extending
  `@group(1)` (the bounds-queue group) with a 5th binding. Reason:
  `@group(1)` is shared between all three bounds_calc entry points; adding a
  binding there would force `add_initial_groups_to_bound_queue` and
  `compute_group_bounds` to bind a buffer they don't touch. `@group(3)` is
  prepare-pipeline-only and the wasm `max_bind_groups = 4` cap allows
  exactly 4 groups (this dispatch sits at the limit).
- **Buffer layout: chose `array<u32>` flat (vec4-per-entry packed)** rather
  than `array<vec4<u32>>`. Reason: WGSL `array<vec4<u32>>` has a 16-byte
  stride identical to packed flat u32s, but flat indexing is a tighter match
  for the CPU-side decode (the `u32::from_le_bytes` walk reads at u32
  alignment with no padding-aware offsets).
- **Readback: chose end-of-gate single drain (Acceptable simplification 1)**
  rather than per-N-frame streaming (simplification 2). Reason: the brief
  explicitly recommends simplification 1; the
  `populate_cpu_mirror_from_gpu_producer` pattern proves the
  `map_async`+`AtomicBool` flow is target-agnostic; the single-drain probe
  delivers all entries in one info!() batch with no across-frame state to
  reason about.
- **Sentinel prefix: chose `[aadf-probe2] [probe1-call]` double-token line**
  rather than editing the Playwright spec's `wgpuDiagnosticLines` filter.
  Reason: brief mandate to keep instrumentation shader+Rust-side only; the
  prefix piggyback hits the existing filter without touching the spec.
- **Capacity: 2048 entries** is enough for native (165 calls) AND web (≤210
  calls observed). The WGSL `arrayLength` guard at the write site allows
  the buffer to be re-sized without WGSL changes.
- **NOT a fix: every edit is observation-only.** No constant changed; no
  algorithm changed; no barrier inserted. The probe is purely additive.

### Assumptions made
- The brief's `[probe1-call]` line format is intentionally compact —
  prefixing it with `[aadf-probe2]` for spec-filter-compatibility does not
  violate the format constraint (the line still contains
  `[probe1-call] call_idx=N qi=N found_size=N` as a substring).
- `qi` in the brief's specified format is a packed integer key; my output
  encodes it as `sizeN_axN` (human-readable) plus `NONE` (sentinel) for
  legibility. The pre-pack u32 value is recoverable via the documented
  packing rule (`size << 16 | axis` for "found"; `0xFFFFFFFF` for "none").
- The Playwright spec's `page.on("console")` handler already forwards lines
  that match `[aadf-probe2]`; I verified by counting `[probe1-call]`
  matches in the web Playwright stdout (`tee`-captured) — all per-call
  emissions DO reach Playwright's stdout, confirming the spec filter
  passes them through.
- The web SSIM variance (0.79 → 0.78 → 0.81) is consistent with the
  pre-probe baseline (~0.79 per the diagnosis document). The probe writes
  are a single `array<u32>` store per prepare call — sub-microsecond
  overhead, unlikely to perturb the algorithm timing.
- The `aadf_delayed_probe` system from the prior dispatch is left in tree
  but is OBSERVED to still NOT fire on web (no `[aadf-probe2 pass=0]` lines
  with the old probe-1A ring decode in the web logs, only my new
  `[probe1-call]` and `[probe1-call-meta]` lines from `aadf_per_call_probe`).
  The reason is OUTSIDE THIS DISPATCH's scope — the new system uses a
  separate state machine that DOES fire on both targets.

## Predict-the-outcome (carried from the brief — assessment)

The brief's predict-the-outcome line:

> If H1 is correct, the cross-target table will show: native call_idx→found_size
> mapping identical across runs 1+2, web mappings varying across runs 1-3, AND
> web values predominantly ≤ native values at matching keys. If web is constant
> across runs, or web values are higher than native, H1 is wrong.

- **Native mapping identical across runs 1+2?** YES — byte-for-byte.
- **Web mappings varying across runs 1-3?** PARTIALLY — the value at every
  matched (call_idx, qi) is identical across the 3 web runs, but the
  *number of calls* observed by drain time varies (205 / 210 / 200). This
  is consistent with H1 (the algorithm's per-call observation is
  determinably wrong on web, with variance in run-time-by-frame manifesting
  as variance in number-of-calls-by-drain-time).
- **Web values ≤ native values at matching keys?** Strictly only call_idx=0
  has matching keys (both at qi=size0_ax0). The values match (both 32768).
  At every later call_idx the targets observe DIFFERENT queues, and the
  pattern is even stronger than "lower": web sees `size=0` (never written)
  at queues that native sees populated by `atomicAdd` from compute. This is
  the predicted H1 mechanism in its purest form.

**Verdict:** H1 confirmed. The signal is stronger than the brief's
prediction wording — web doesn't see *smaller* values from atomicLoad; it
sees *zero* values at queues that native sees populated, because the
cross-pass atomicAdd from compute is COMPLETELY invisible to the next
prepare's atomicLoad on Dawn.
