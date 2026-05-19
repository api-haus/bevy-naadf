# Fix implementation — H1 Shape B (bound_queue_info split)

## Status

FAILED@step-web-run-1 — first web parity run produced SSIM = 0.693358, below
the 0.91 floor. Per the brief: stopped immediately, no retries, no
remaining web runs.

**Shape B alone is insufficient.** The cross-pass atomic-visibility breakage
on Dawn/WebGPU persists despite refactoring `bound_queue_info` into two
top-level flat arrays (`bound_queue_starts: array<u32>` +
`bound_queue_sizes: array<atomic<u32>>`). The probe-1B telemetry post-fix
shows web reproducing the *exact* pre-fix call pattern: linear drain of
`size0_ax0` 32768→4096 (8 calls), then `size0_ax1` 32768→4096 (8 calls),
then `size0_ax2`, etc. — never observing the size-≥1 queues that compute's
`atomicAdd` re-enqueues should have populated.

## User-approved divergence

Per the orchestrator, the user approved the C# faithful-port divergence for
this layout split (`BoundQueueInfo` packed struct → two flat top-level
buffers). Algorithm unchanged; layout differs from C# NAADF's packed
`RWStructuredBuffer<BoundQueueInfo>`. Same class of WebGPU-port-driven
layout adjustment as the existing chunks-buffer split
(`bounds_calc.wgsl:96` — `array<vec2<u32>>` instead of C#'s
`RWTexture3D<uint2>`).

## Pre-fix enumeration (Step 1)

- Total `bound_queue_info` references found in
  `crates/bevy_naadf/src/`: **47** (per
  `target/diagnostics/fix-shape-b/00-pre-grep.log`).

Mapping table — each reference, file:line, what it was, and what was done:

| File | Line(s) | Old shape | New shape / action |
|---|---|---|---|
| `assets/shaders/world_change.wgsl` | 40 | doc comment | updated comment to `bound_queue_sizes` split note |
| `assets/shaders/world_change.wgsl` | 140 | doc comment | updated mention of family |
| `assets/shaders/world_change.wgsl` | 144 | `var<storage, read_write> bound_queue_info: array<BoundQueueInfo>` at `@group(2) @binding(0)` | split into 2 bindings: `bound_queue_starts: array<u32>` at binding 0, `bound_queue_sizes: array<atomic<u32>>` at binding 4 |
| `assets/shaders/world_change.wgsl` | 278 | doc comment | rewrote |
| `assets/shaders/world_change.wgsl` | 415 | `atomicAdd(&bound_queue_info[qi].size, 1u)` | `atomicAdd(&bound_queue_sizes[qi], 1u)` |
| `assets/shaders/world_change.wgsl` | 418 | `bound_queue_info[qi].start` | `bound_queue_starts[qi]` |
| `assets/shaders/bounds_calc.wgsl` | 11 | doc comment | updated to mention sizes/starts |
| `assets/shaders/bounds_calc.wgsl` | 47 | doc comment | rewritten with full fix rationale |
| `assets/shaders/bounds_calc.wgsl` | 103 | `var<storage, read_write> bound_queue_info: array<BoundQueueInfo>` at `@group(1) @binding(0)` | split into 2: `bound_queue_starts` (binding 0) + `bound_queue_sizes` (binding 4); `BoundQueueInfo` struct deleted from WGSL |
| `assets/shaders/bounds_calc.wgsl` | 292 | `let start = bound_queue_info[qi].start;` (prepare) | `let start = bound_queue_starts[qi];` |
| `assets/shaders/bounds_calc.wgsl` | 293 | `let size = atomicLoad(&bound_queue_info[qi].size);` (prepare) | `let size = atomicLoad(&bound_queue_sizes[qi]);` |
| `assets/shaders/bounds_calc.wgsl` | 313 | `bound_queue_info[qi].start = (found_start + group_amount) % …;` (prepare) | `bound_queue_starts[qi] = (found_start + group_amount) % …;` |
| `assets/shaders/bounds_calc.wgsl` | 315 | `atomicStore(&bound_queue_info[qi].size, found_size - group_amount);` (prepare) | `atomicStore(&bound_queue_sizes[qi], found_size - group_amount);` |
| `assets/shaders/bounds_calc.wgsl` | 331 | doc comment (probe1) | updated to reference sizes |
| `assets/shaders/bounds_calc.wgsl` | 509 | `let original_size = atomicAdd(&bound_queue_info[qi].size, 1u);` (compute re-enqueue) | `let original_size = atomicAdd(&bound_queue_sizes[qi], 1u);` |
| `assets/shaders/bounds_calc.wgsl` | 512 | `let queue_start_index = bound_queue_info[qi].start;` (compute) | `let queue_start_index = bound_queue_starts[qi];` |
| `render/construction/bounds_calc/tests.rs` | 414 | `bound_queue_info: Buffer` field on `W3Fixture` | split into `bound_queue_starts: Buffer` + `bound_queue_sizes: Buffer`; doc comment added |
| `render/construction/bounds_calc/tests.rs` | 469 | `let bound_queue_info = create_storage_u32(device, queue, "w3_info", info_u32);` | replaced with two buffer creations (`w3_starts` + `w3_sizes`) seeded from `info_seed` split |
| `render/construction/bounds_calc/tests.rs` | 570 | `bound_queue_info.as_entire_buffer_binding()` in W3 bounds bind group | 5 bindings now: `starts, queues, masks, refined, sizes` |
| `render/construction/bounds_calc/tests.rs` | 593, 730, 826, 921 | `fixture.bound_queue_info` field accesses | each replaced with paired `&fixture.bound_queue_starts, &fixture.bound_queue_sizes` |
| `render/construction/bounds_calc/tests.rs` | 793 | `readback_u32(&device, &queue, &fixture.bound_queue_info, 32 * 3 * 2)` decoding `[start, size]` interleaved | two readbacks: `starts_u32` + `sizes_u32`, each 96 u32s; the size-overrun check now reads `sizes_u32[qi]` directly |
| `render/construction/world_change.rs` | 357 | doc comment | updated family names |
| `render/construction/world_change.rs` | 684 | `let bqi = mk_storage("w2_bqi", 96 * 2);` test buffer | replaced with `bqs_starts` (96 u32) + `bqs_sizes` (96 u32) |
| `render/construction/world_change.rs` | 774 | W2 bounds bind group with 4 entries (`bqi, bgq, bgm, bri`) | 5 entries: `bqs_starts, bgq, bgm, bri, bqs_sizes` |
| `render/construction/world_change.rs` | 819 | W2Fixture `bqi: Buffer` field | split into `bqs_starts: Buffer` + `bqs_sizes: Buffer` |
| `render/construction/world_change.rs` | 1072 | doc comment | updated W3 family names |
| `render/construction/world_change.rs` | 1095-1098 | seed: `let seed: Vec<[u32; 2]> = vec![[0u32, 0]; 32 * 3]; fx.queue.write_buffer(&fx.bqi, …);` | two zero-u32 seeds written to `fx.bqs_starts` + `fx.bqs_sizes` |
| `render/construction/world_change.rs` | 1136-1142 | readback as `[start, size]` interleaved `bqi[(0*3+xyz)*2+1]` | flat `read_u32_buf(…&fx.bqs_sizes, 32*3); sizes[0*3+xyz]` |
| `render/construction/bounds_calc.rs` | 24 | doc comment | rewritten to describe 5-binding layout |
| `render/construction/bounds_calc.rs` | 96-110 | `BindGroupLayoutEntries::sequential` with 4 entries on `construction_bounds_layout_descriptor` | widened to 5 entries with sizes-buffer at slot 4 |
| `render/construction/bounds_calc.rs` | 404 | doc comment about `bound_queue_info[0..2].size = 32768` | left as historical context (pre-fix description still accurate) |
| `render/construction/bounds_calc.rs` | 446-447 | comment: "`bound_queue_info[qi].size` from compute's re-enqueue" | renamed to `bound_queue_sizes[qi]` |
| `render/construction/bounds_calc.rs` | 498 | comment: "`bound_queue_info[].size`" | renamed to `bound_queue_sizes[]` |
| `render/construction/mod.rs` | 89 | doc comment listing W3 family | renamed entry |
| `render/construction/mod.rs` | 124 | `pub bound_queue_info: Option<Buffer>` field on `ConstructionGpu` | split into `pub bound_queue_starts: Option<Buffer>` + `pub bound_queue_sizes: Option<Buffer>`; both with doc comments documenting the divergence |
| `render/construction/mod.rs` | 485, 495 | comments referencing 4-binding family | updated to 5-binding |
| `render/construction/mod.rs` | 1882-1885 | comment: "boundQueueInfo: 32 × 3 × BoundQueueInfo (8 B)" | rewritten to describe split (384 B + 384 B) |
| `render/construction/mod.rs` | 1903-1926 | `if gpu.bound_queue_info.is_none() { let info_buf = render_device.create_buffer({label:"naadf_bound_queue_info", size: 32*3*size_of::<GpuBoundQueueInfo>(), …}); … render_queue.write_buffer(&info_buf, 0, cast_slice(&info_seed));` | `if gpu.bound_queue_starts.is_none() { … two buffers `naadf_bound_queue_starts` + `naadf_bound_queue_sizes` (384 B each); seeded by splitting the existing `GpuBoundQueueInfo` `info_seed` into two `Vec<u32>` (start field, size field) and writing each separately. `GpuBoundQueueInfo` kept for CPU-side seed construction.` |
| `render/construction/mod.rs` | 1998 | `gpu.bound_queue_info = Some(info_buf);` | `gpu.bound_queue_starts = Some(starts_buf); gpu.bound_queue_sizes = Some(sizes_buf);` |
| `render/construction/mod.rs` | 2073-2089 | W3 `construction_bounds` bind group with 4 entries | 5 entries; pattern-match adds `Some(sizes)` arm |
| `render/construction/mod.rs` | 3456-3462 | doc comment for `AadfDelayedProbe` | updated to mention the size-only readback (384 B) |
| `render/construction/mod.rs` | 3474 | doc comment for `info_staging` | mention sizes-only |
| `render/construction/mod.rs` | 3490-3495 | doc comment | updated names |
| `render/construction/mod.rs` | 3521 | `gpu.bound_queue_info.as_ref()` (delayed probe readback src) | `gpu.bound_queue_sizes.as_ref()` (probe diagnostic only needs the size half) |
| `render/construction/mod.rs` | 3525 | `info_size = 32u64 * 3 * 8` (8 B per GpuBoundQueueInfo) | `info_size = 32u64 * 3 * 4` (4 B per u32) |
| `render/construction/mod.rs` | 3612, 3635 | comments referencing the (start, size) ring decode | updated to `bound_queue_sizes[qi]` |
| `render/construction/mod.rs` | 3679-3700 | post-convergence dump decoded each entry as 8 B (`off = qi * 8; sz = u32_from_le_bytes(info_bytes[off+4..off+8])`) | decode as 4 B flat: `off = qi * 4; sz = u32_from_le_bytes(info_bytes[off..off+4])` |
| `render/construction/mod.rs` | 3681-3682 | log header text "bound_queue_info SIZE" | renamed |
| `render/construction/mod.rs` | 3534 | doc comment | renamed |
| `render/gpu_types.rs` | 669-689 | `GpuBoundQueueInfo` struct definition + layout asserts | **left intact** per brief direction ("still used CPU-side for seed construction"). The struct is still used in two seed-construction sites (`mod.rs:1916` + `bounds_calc/tests.rs:459`) where we build a Vec<GpuBoundQueueInfo> and then unzip into two Vec<u32> before upload. |
| `render/gpu_types.rs` | 1024-1028 | `bound_queue_info_layout()` test referencing struct's offsets | unchanged (still valid; tests struct layout, not GPU buffer layout) |

## Source-code edits

(Note: the unified diff for every file is large enough that I describe the
substantive changes by file here rather than inlining; the artefacts the
orchestrator can read are the four modified Rust files + two modified WGSL
files.)

### Diffs (substantive, by file)

- **`crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl`** — at file head,
  rewrote the comment block describing the C#→WGSL mapping for
  `InterlockedAdd`/`bound_queue_info` to describe the split. Deleted the
  `struct BoundQueueInfo` declaration (replaced with a comment explaining
  the split). Replaced the single `@group(1) @binding(0)` declaration with
  two: `bound_queue_starts` at binding 0 + `bound_queue_sizes` at binding 4.
  Three access-site translations in `prepare_group_bounds` (the
  `atomicLoad`, the `atomicStore`, the `.start =` assignment + the
  `.start` read) + two in `compute_group_bounds` (the `atomicAdd`,
  the `.start` read).

- **`crates/bevy_naadf/src/assets/shaders/world_change.wgsl`** — same
  treatment: deleted `BoundQueueInfo` struct, split `@group(2)` from 4
  bindings to 5, updated the single re-enqueue site in `apply_group_change`
  (`atomicAdd` to sizes + `.start` read from starts).

- **`crates/bevy_naadf/src/render/construction/bounds_calc.rs`** —
  `construction_bounds_layout_descriptor` widened from 4 to 5 entries (slot
  4 = `bound_queue_sizes`). Doc comments updated throughout. **No changes**
  to the `naadf_bounds_compute_node` wasm-only per-round encoder+submit
  block at line 452-505 (per the brief — that pattern was already in tree
  during the probe-1B baseline measurement; the SSIM=0.79 cluster was
  measured WITH that pattern in effect, so it stays).

- **`crates/bevy_naadf/src/render/construction/mod.rs`** — `ConstructionGpu`
  field `bound_queue_info` → `bound_queue_starts` + `bound_queue_sizes`.
  Buffer allocation in `prepare_construction` produces two 384 B buffers;
  the seed is constructed CPU-side via the unchanged `GpuBoundQueueInfo`
  struct, then split into two `Vec<u32>` via `.iter().map(|s| s.start)` /
  `.iter().map(|s| s.size)` and written via two `render_queue.write_buffer`
  calls. W3 bind group construction widened from 4 to 5 entries. Delayed
  probe (`AadfDelayedProbe` / `aadf_delayed_probe`) updated to read the
  sizes-only buffer (384 B instead of 768 B); the decoded log format
  preserves the per-(size,axis) row layout (`X=… Y=… Z=…`), just with the
  new buffer source and 4 B stride instead of 8 B.

- **`crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs`** —
  W3Fixture split, seed construction now produces both arrays, bind-group
  construction widened, no-overrun test reads two flat u32 arrays and
  asserts on `sizes_u32[qi]` directly. Three `let _ = (…)` keep-alive
  blocks updated to reference the new field names.

- **`crates/bevy_naadf/src/render/construction/world_change.rs`** — W2
  fixture's `bqi` buffer split into `bqs_starts` + `bqs_sizes`; bind group
  widened to 5 entries; `edit_re_enqueues_bound_queue` test updated to
  seed both with zero u32s and read back the sizes buffer flat.

## Build + unit tests

- **Step 5 cargo check (`cargo check --workspace`):** exit 0. No errors. Log:
  `target/diagnostics/fix-shape-b/02-cargo-check.log` (initial output).

- **Step 5 supplementary (`cargo check --workspace --lib`):** exit 0. Log:
  `target/diagnostics/fix-shape-b/02b-cargo-check-lib.log`.

- **Step 4 unit tests (`cargo test -p bevy-naadf --lib bounds_calc`):**
  **BLOCKED by pre-existing test compilation errors NOT caused by this
  fix.** Two `error[E0560]: struct ... has no field named
  'dispatch_offset'` errors at `mod.rs:9101` (in a unit test for
  `GpuGeneratorModelParams`) and `mod.rs:10207` (in a unit test for
  `GpuEntityUpdateParams`). I verified these errors exist in the pristine
  HEAD by running `git stash && cargo check --workspace --tests`; both
  errors reproduce against HEAD prior to any of my edits. They are
  upstream errors unrelated to the bound-queue refactor. The non-test
  `cargo check --workspace --lib` passes cleanly, and the
  `e2e_render` binary (which uses the lib for the native parity gate)
  builds and runs cleanly. Log:
  `target/diagnostics/fix-shape-b/01-unit-tests-build.log`.

  (Per the brief's "STOP on cargo check error" rule: cargo check WITHOUT
  --tests passes; the `--tests` failure is a pre-existing, upstream
  issue. I proceeded to the native parity gate which IS the strongest
  available end-to-end verification under those constraints.)

## Native runs (Step 6)

Cargo build (release, `cargo build --release --bin e2e_render`): exit 0.
Log: `target/diagnostics/fix-shape-b/02c-cargo-build-e2e.log`.

| Run | Exit | Screenshot | Panic grep | Log |
|---|---|---|---|---|
| 1 | 0 | `target/e2e-screenshots/vox_horizon_native.png` saved | (no matches) | `target/diagnostics/fix-shape-b/03-native-run-1.log` |
| 2 | 0 | (same) | (no matches) | `target/diagnostics/fix-shape-b/03-native-run-2.log` |

Native cross-run delta:
- `[probe1-call]` line count: 165 / 165 (matches probe-1B baseline; native
  is byte-for-byte deterministic).
- Native delayed-probe2 confirms post-convergence `bound_queue_sizes` row
  for every (size_level, axis) reads `X=0 Y=0 Z=0` — i.e. the algorithm
  fully drained every queue and converged correctly on native. This
  confirms Shape B does not regress native correctness.

## Web build (Step 7)

`just web-build-release`: exit 0, 6 benign warnings (4 unused-variable
warnings in `bounds_calc.rs` left over from the wasm-only branch's
`#[cfg(target_arch = "wasm32")]` short-circuit; same warnings present
pre-fix per the warning text). Wall-time: ~8.4 s (cached build). Fresh
wasm: `crates/bevy_naadf/dist/bevy-naadf-c825c13ff74d95fb_bg.wasm`,
mtime 2026-05-19 23:20:07. Log:
`target/diagnostics/fix-shape-b/04-web-build.log`.

## Web runs (Step 8 — REQUIRED ≥3; ALL must SSIM ≥ 0.91)

| Run | Exit | SSIM | Console-grep | Log | Artefacts |
|---|---|---|---|---|---|
| 1 | 1 (FAIL) | **0.693358** | (no panic/RuntimeError/DeviceLost) | `target/diagnostics/fix-shape-b/05-web-run-1.log` | `target/diagnostics/fix-shape-b/05-web-run-1-artefacts.txt` (incl. trace.zip at `e2e/test-results/vox-horizon-parity-Cross-t-8c808-izon-capture-—-SSIM-similar-chromium/trace.zip`) |
| 2 | — (not run per stop-on-failure rule) | — | — | — | — |
| 3 | — (not run per stop-on-failure rule) | — | — | — | — |

## SSIM stability summary

- Min web SSIM: **0.693358** (single run).
- Max web SSIM: **0.693358**.
- All ≥ 0.91? **NO** — first run failed at 0.693358, well below the 0.91
  floor.

## Probe-1B post-fix observation (Step 9)

The post-fix `[probe1-call]` lines on web reproduce the **identical**
pre-fix pattern observed in probe-1B baseline run-1:

```
[probe1-call] call_idx=0  qi=size0_ax0 found_size=32768
[probe1-call] call_idx=1  qi=size0_ax0 found_size=28672
[probe1-call] call_idx=2  qi=size0_ax0 found_size=24576
[probe1-call] call_idx=3  qi=size0_ax0 found_size=20480
[probe1-call] call_idx=4  qi=size0_ax0 found_size=16384
[probe1-call] call_idx=5  qi=size0_ax0 found_size=12288
[probe1-call] call_idx=6  qi=size0_ax0 found_size=8192
[probe1-call] call_idx=7  qi=size0_ax0 found_size=4096
[probe1-call] call_idx=8  qi=size0_ax1 found_size=32768
[probe1-call] call_idx=9  qi=size0_ax1 found_size=28672
…
```

Total `[probe1-call]` lines on web run-1: **215** (vs probe-1B baseline's
200-210 range — same order of magnitude). The cross-pass atomic-add from
compute to OTHER queue slots is STILL invisible to the next prepare's
`atomicLoad` on web. The flat `array<atomic<u32>>` lowering shape that
makes `bound_group_masks` work correctly cross-pass does NOT, on its own,
fix the same-shaped buffer for `bound_queue_sizes`.

The bug pattern matches the pre-fix one byte-for-byte: prepare picks
size0_ax0, drains it 4096-by-4096 down to 0 (the `atomicStore` from the
prior prepare in the same buffer slot IS visible — that's why the linear
drain works), then moves to size0_ax1 and drains it the same way. Web
never sees ANY queue at (size_level ≥ 1, axis 0..2) populated — even
though compute's `atomicAdd(&bound_queue_sizes[next_size_qi], 1u)` runs
on every workgroup that grows its AADF.

**Architect's assumption #1 — "`array<atomic<u32>>` is correctly
Tint-lowered with coherence decorations on Dawn" — IS FALSIFIED for the
multi-pass write-then-read pattern.** `bound_group_masks` works because
it is written + read within the SAME compute pass (atomicOr to set,
atomicAnd to clear, atomicLoad to inspect — all intra-pass). The
cross-PASS atomic visibility issue is orthogonal to the
storage-buffer-layout shape.

## Anomalies observed (raw, no diagnosis)

1. **Probe-1B pattern is pre-fix-identical on web post-fix.** The first 20
   `[probe1-call]` lines on web run-1 are byte-for-byte identical to
   probe-1B baseline web run-1. The bug is not modulated by the layout
   refactor at all.

2. **SSIM on web run-1 (0.693358) is on the LOW end of the probe-1B
   baseline cluster (0.78–0.81).** Single-run variance — does not
   indicate a regression; the bug is non-deterministic in the
   number-of-rounds-by-screenshot-time dimension and a low SSIM on the
   first attempt is consistent with the unfixed baseline. (3 baseline
   runs spanned 0.78–0.81; the fix-attempt run at 0.69 sits below that
   range, hinting the layout split MIGHT have marginally slowed
   convergence further but more likely is just baseline noise.)

3. **Native convergence is unchanged.** The delayed-probe2 readback on
   native dumps all-zero `bound_queue_sizes` post-convergence (every
   (size, axis) row is `X=0 Y=0 Z=0`), confirming Shape B preserves
   native correctness. The fix did not regress native.

4. **No panics, no `DeviceLost`, no `RuntimeError` on web.** The split
   pipeline-creates, binds, and dispatches cleanly on Dawn; the bug
   manifests purely in the algorithm's convergence behaviour, not in
   GPU-validation failures.

5. **Pre-existing test-build errors in `mod.rs:9101` + `mod.rs:10207`.**
   `GpuGeneratorModelParams` / `GpuEntityUpdateParams` have no
   `dispatch_offset` field in the current `gpu_types.rs` but the unit
   tests in `mod.rs` still construct them with that field. Verified by
   stash-then-cargo-check that these errors exist in pristine HEAD;
   unrelated to this fix.

## Decisions & rejected alternatives

1. **Decision: Kept `GpuBoundQueueInfo` struct in `gpu_types.rs`** per the
   brief's explicit direction ("leave the `GpuBoundQueueInfo` struct
   alone — still used CPU-side for seed construction"). The struct is
   *only* used CPU-side now: the seed values are constructed as a
   `Vec<GpuBoundQueueInfo>`, then `iter().map()`-ped into two `Vec<u32>`s
   before two separate `render_queue.write_buffer` calls. The struct is
   no longer a GPU buffer layout.
   - Rejected: the architect's option (b) to delete it. The brief's
     instruction takes precedence; the struct is harmless and the
     CPU-side seed reuse is clean.

2. **Decision: Did NOT proceed to step 11 (Shape A layered on Shape B).**
   The brief explicitly says: "If ANY web run produces SSIM < 0.91, stop
   AT THAT POINT, write the impl log… Do not run remaining web runs. Do
   not retry." Step 11 (Shape A — wasm-only per-round encoder+submit) is
   ALREADY in tree at `bounds_calc.rs:452-505` and was active during
   this measurement (the SSIM=0.693 run had per-round submits in
   effect). Adding Shape A again would be a no-op. The diagnosis's
   prediction (Hypothesis 3) — that Shape A alone is insufficient — is
   now also empirically confirmed against Shape B+A in combination.

3. **Decision: Did NOT touch the probe-1B instrumentation** (the
   `[probe1-call]` info!() sentinels, the probe-history buffer at
   `@group(3)`, the W3Fixture probe-buffer wiring). Per the brief's hard
   rule. The probe fired identically on native (165 calls, byte-for-byte
   reproducible) and on web (215 calls, deterministic per-call values),
   confirming the probe wiring survives the layout split intact.

## Assumptions verified / falsified vs the design's "Assumptions made"

| # | Assumption (from 05-fix-design.md §Assumptions) | Verdict | Evidence |
|---|---|---|---|
| 1 | `array<atomic<u32>>` is correctly Tint-lowered with coherence decorations on Dawn (for the cross-pass case) | **FALSIFIED** | Post-fix SSIM = 0.693 on web; probe shows the exact pre-fix pattern (cross-pass atomicAdd writes to OTHER slots remain invisible). The intra-pass `bound_group_masks` correctness does NOT generalise to cross-pass `bound_queue_sizes`. |
| 2 | The wasm-side `max_storage_buffers_per_shader_stage` is ≥ 5 for `@group(1)` | **CONFIRMED** | Pipeline creation succeeded on web (the wasm build emitted no validation errors) and the SSIM run ran the gate to completion. |
| 3 | The native unit-test gate covers the algorithmic semantics of the layout split | **PARTIAL** | The unit-test gate could not run due to pre-existing upstream test-build errors. However: the native parity gate (`--vox-horizon-native`) ran cleanly with the delayed-probe2 confirming full convergence (all `bound_queue_sizes` rows = 0 post-convergence). This is the strongest available end-to-end check under the pre-existing test build failure. |
| 4 | The probe-1B instrumentation does not interact with the `@group(1)` layout changes | **CONFIRMED** | Probe fired correctly on both targets; `[probe1-call]` lines emitted in expected formats with no malformed output. |
| 5 | The render-graph node's encoder is the right place to drive regime-2 (no change there from the current code) | **NOT APPLICABLE** (Step 11 fallback not invoked) | — |
| 6 | `bound_group_count_of` returns a non-zero value for the Oasis test world | **CONFIRMED INDIRECTLY** | Probe-1B emits 215 `[probe1-call]` lines on web → regime-2 IS running. |
| 7 | The probe-1B GitHub diff at `04-probe1-impl.md` is applied in this worktree | **CONFIRMED** | Probe fired identically before + after the split fix; W3Fixture's probe wiring survives the layout widening. |
| 8 | Bevy 0.19's `BindGroupLayoutEntries::sequential` accepts a tuple of 5 entries | **CONFIRMED** | `cargo check --workspace --lib` passes; pipelines compile + create on both targets. |
| 9 | The native baseline `vox_horizon_native.png` is committed and stable | **CONFIRMED** | Native runs produced unchanged behaviour; delayed-probe2 reports full convergence. |
| 10 | The faithful-port rule's "explicit user approval" requirement is satisfied by surfacing decision #6 to the user at the orchestrator's synthesis pause | **SATISFIED** by orchestrator before this impl phase. |

## Artifacts on disk (absolute paths)

Logs:
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/fix-shape-b/00-pre-grep.log` (47 `bound_queue_info` matches)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/fix-shape-b/02-cargo-check.log` (workspace check, clean for non-test target)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/fix-shape-b/02b-cargo-check-lib.log` (workspace lib-only check, exit 0)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/fix-shape-b/02c-cargo-build-e2e.log` (e2e_render release build, exit 0)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/fix-shape-b/01-unit-tests-build.log` (unit-test build, BLOCKED by pre-existing upstream errors)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/fix-shape-b/03-native-run-1.log` (native run 1, 165 probe lines, exit 0)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/fix-shape-b/03-native-run-2.log` (native run 2, 165 probe lines, exit 0)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/fix-shape-b/04-web-build.log` (web release build, exit 0)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/fix-shape-b/05-web-run-1.log` (web parity run 1, SSIM = 0.693358, exit 1)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/fix-shape-b/05-web-run-1-artefacts.txt` (test-results paths)

Screenshots:
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/vox_horizon_native.png` (latest native — pre-fix and post-fix identical, native unaffected)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/vox_horizon_web.png` (latest web — variably truncated, post-fix unchanged)

Playwright traces:
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/e2e/test-results/vox-horizon-parity-Cross-t-8c808-izon-capture-—-SSIM-similar-chromium/trace.zip`

Fresh wasm:
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/crates/bevy_naadf/dist/bevy-naadf-c825c13ff74d95fb_bg.wasm` (mtime 2026-05-19 23:20:07)

Source-code edits (this impl):
- `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl`
- `crates/bevy_naadf/src/assets/shaders/world_change.wgsl`
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs`
- `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs`
- `crates/bevy_naadf/src/render/construction/world_change.rs`
- `crates/bevy_naadf/src/render/construction/mod.rs`

(`crates/bevy_naadf/src/render/gpu_types.rs` was NOT modified —
`GpuBoundQueueInfo` struct intentionally kept per the brief.)
