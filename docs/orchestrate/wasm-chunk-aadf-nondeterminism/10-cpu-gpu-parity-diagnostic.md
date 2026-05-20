# CPU-vs-GPU chunks bit-parity diagnostic — wasm

## Question
Does the CPU-side AADF oracle produce bit-identical chunks to the GPU-built
chunks on wasm32/WebGPU?

## Predict-the-outcome (locked before web runs)
**Predicted: web `ratio<100%` — chunks ARE wrong on the failing runs;
consistent with `09-synthesis-classifications.md` §F's chunks-AADF asymmetry
finding (`[0,0,0,0,1,1]` vs native `[4,4,3,3,3,3]`) and `04-probe1-impl.md`'s
verdict that web's regime-2 algorithm is deterministically broken.** Specifically:
- Failing web runs (SSIM ≈ 0.69-0.81) should show ratio well below the native
  100% baseline; the broken regime-2 leaves chunks AADFs unexpanded.
- The variance across web runs (which the synthesis frames as the timing of
  Dawn's batched-flush threshold relative to screenshot) should manifest as
  variance in the ratio: lower ratio on FAIL runs, higher on the rare PASS
  ("lucky") runs.

## Instrumentation summary
- **Bevy system added:** `aadf_cpu_gpu_parity` at
  `crates/bevy_naadf/src/render/construction/mod.rs:4111-4561`
  (ExtractSchedule, render world).
- **CPU oracle:** inline port of `aadf::bounds::compute_aadf_layer`
  (`crates/bevy_naadf/src/aadf/bounds.rs:247-335`) with one change — when an
  axial neighbour is OUT OF BOUNDS, the oracle treats it as "empty with all
  six AADF distances at `AADF_MAX_CHUNK = 31`". This matches the GPU's
  boundary semantic ("OOB = empty, free-expanding"); the upstream
  `compute_aadf_layer` clamps at `[0, dims-1]` instead, which deterministically
  diverges from the GPU at all chunks within 31 cells of any world boundary.
  Confirmed via the native baseline (Step 2): plain `compute_aadf_layer`
  gave ratio=81.3% on native; the OOB-extended port gives ratio=100%.
- **GPU readback source:** fresh `copy_buffer_to_buffer` of
  `WorldGpu::chunks_buffer` into a MAP_READ staging buffer
  (`mod.rs:4154-4172`). Uses the same proven-on-web `map_async + AtomicBool`
  pattern that drives `populate_cpu_mirror_from_gpu_producer`. The chunks
  buffer is `array<vec2<u32>>` stride 8 B; the diagnostic reads the lower
  `u32` (`.x` = chunk AADF carrier per `bounds_calc.wgsl:538`).
- **Trigger:** `cpu_mirror_populated && frames_since_mirror >=
  PARITY_TRIGGER_FRAMES = 60` (`mod.rs:4078`). ~1 s @ 60 fps post-mirror —
  double the probe-1B `PROBE_TRIGGER_FRAMES` of 30 to give the regime-2 loop
  more time to converge.
- **Output sentinels:**
  - `[aadf-probe2] [cpu-gpu-parity-meta] readback ISSUED ...` — emitted at
    issue time so we can tell from logs that the trigger fired.
  - `[aadf-probe2] [cpu-gpu-parity] same_bytes=N total_bytes=M ratio=R% ...`
    — the headline ratio line + variants restricted to interior chunks
    (excluding the 1-deep world-edge ring, just for additional context).
  - `[aadf-probe2] [cpu-gpu-parity-diff] rank=K chunk_idx=N chunk_pos=(x,y,z)
    boundary=B oracle=[...] gpu=[...]` — first 10 differing chunks.
  - `[aadf-probe2] [cpu-gpu-parity-interior-diff] rank=K ...` — first 10
    differing INTERIOR chunks (chunks not on any world boundary).
  - `[aadf-probe2] [cpu-gpu-parity-meta] DONE` — terminal.

  The `[aadf-probe2]` prefix piggybacks on the existing Playwright spec
  filter at `e2e/tests/vox-horizon-parity.spec.ts:225-237` so the line
  reaches Playwright's stdout pipe.
- **Native artifact dump (gated `#[cfg(not(target_arch = "wasm32"))]`):**
  writes `target/diagnostics/cpu-gpu-parity/native-cpu-chunks.bin` (the CPU
  oracle's chunk words) and `native-gpu-chunks.bin` (the GPU's chunk lower
  `u32`s) for offline byte-compare.

## Source-code edits

| File | Lines | Purpose |
|---|---|---|
| `crates/bevy_naadf/src/render/construction/mod.rs` | +485 (new `AadfCpuGpuParity` resource, `CpuGpuParityStage` enum, `PARITY_TRIGGER_FRAMES` const, `aadf_cpu_gpu_parity` system body), +1 (`.init_resource::<AadfCpuGpuParity>()`), +7 (`.add_systems(ExtractSchedule, aadf_cpu_gpu_parity)` registration) | the entire CPU-vs-GPU parity diagnostic. |

No other files edited. Probe-1B per-call instrumentation, the wasm-only
per-round-encoder branch at `bounds_calc.rs:365-417`, the
`WASM_MAX_GROUP_BOUND_DISPATCH = 4096` cap, the `HORIZON_SSIM_SIMILARITY_MIN`
constant, and `MAX_RAY_STEPS_PRIMARY` are all unchanged.

## Native baseline (sanity, 2 runs)

Both runs `--vox-horizon-native` (= `cargo run --release --bin e2e_render --
--vox-horizon-native`). 256×32×256 fixed world; oasis.cvox loaded; ran the
CPU oracle in-process against the GPU's converged chunks buffer at frame 60
post-`cpu_mirror_populated`.

| Run | Exit | [cpu-gpu-parity] same_bytes | total_bytes | ratio | interior_ratio | First 10 differing chunk_idx |
|---|---|---|---|---|---|---|
| native-1 | 0 | 10460124 | 10460124 | **100.000%** | 100.000% | (none) |
| native-2 | 0 | 10460124 | 10460124 | **100.000%** | 100.000% | (none) |

- All 1,743,354 empty chunks match across all 6 axes; all 9,513,150 interior
  AADF distance bytes match.
- Cross-run `cmp(native-run-{1,2}-{cpu,gpu}-chunks.bin)` returns equality
  for both pairs — native is fully byte-deterministic across runs.

**Sanity result: 100% match on native — diagnostic is meaningful.**

## Web runs (3, all `cd e2e && timeout 240s npx playwright test
vox-horizon-parity.spec.ts --headed`)

Both 256×32×256 fixed world; oasis.cvox loaded via the live wasm path; the
parity readback fires through the same state machine as on native. Logs are
under `target/diagnostics/cpu-gpu-parity/web-run-{1,2,3}.log`; the
`[cpu-gpu-parity]` lines are forwarded to Playwright stdout via the spec's
`[aadf-probe2]` filter.

| Run | Exit | SSIM | [cpu-gpu-parity] same_bytes | total_bytes | ratio | interior_ratio | empty_all_axes_match | First 10 differing chunk_idx |
|---|---|---|---|---|---|---|---|---|
| web-1 | 1 | 0.692655 | 433783 | 10460124 | **4.147%** | 4.503% | 1272 / 1,743,354 | 0,1,2,3,4,5,6,7,8,9 (boundary); interior 8449,8450,8451,8452,8453,8454,8455,8456,8457,8458 |
| web-2 | 0 (PASS) | ≥ 0.91 | 3847821 | 10460124 | **36.786%** | 37.921% | 192829 / 1,743,354 | same chunk_idx as web-1 but with PARTIAL gpu AADFs (`gpu=[0,0,15,3,14,14]` instead of all-zero) |
| web-3 | 1 | 0.693403 | 433783 | 10460124 | **4.147%** | 4.503% | 1272 / 1,743,354 | identical to web-1 |

### Per-run GPU value patterns for the first 10 INTERIOR diffs

All three runs sample the same chunk_idx slots (8449, 8450, ..., 8458),
chunk_pos (1,1,1) to (10,1,1). The oracle returns position-dependent
expansion (`oracle=[31,14,31,3,31,18]` for (1,1,1), decreasing in mx as x
grows). The GPU values reveal the bug class:

| run | gpu @ (1,1,1) | gpu @ (5,1,1) | gpu @ (10,1,1) | pattern |
|---|---|---|---|---|
| native | [31,14,31,3,31,18] | [31,13,31,3,31,10] | [31,8,31,3,31,10] | ORACLE MATCH — multi-axis expanded |
| web-1 | [0,0,0,0,0,0] | [0,0,0,0,0,0] | [0,0,0,0,0,0] | **ZERO — not expanded at all** |
| web-2 | [0,0,15,3,14,14] | [0,0,15,3,14,14] | [0,0,15,3,14,14] | partial — Y±/Z± expanded but X axis still 0 |
| web-3 | [0,0,0,0,0,0] | [0,0,0,0,0,0] | [0,0,0,0,0,0] | identical to web-1 |

## Cross-run variance on web
- **Same_bytes count varies across the 3 web runs?** YES — web-1 and web-3
  are byte-identical at 433,783; web-2 is dramatically higher at 3,847,821.
  The variance is bimodal: 2/3 runs land at the "completely-unexpanded"
  baseline; 1/3 runs land at a partially-expanded state. This matches the
  earlier `IMPLEMENTORS_SHARED.md` observation of a 1-in-N "lucky" run.
- **Same SET of differing chunk_idx across runs, or different chunks each
  time?** SAME chunk_idx in EVERY run. The first 10 interior diffs span the
  same 10 slots (chunk_idx 8449..8458, chunk_pos (1..10,1,1)) in all three
  runs. What varies is the GPU value at those slots, not WHICH slots differ.
- **Implication:** the GPU error is at deterministic chunk positions across
  every web run. Combined with web-1 vs web-3 being byte-identical, the
  algorithm's broken trajectory through state-space is deterministic. The
  "non-determinism" the prior dispatches observed is a bimodal outcome —
  either the algorithm runs the deterministically-broken path (web-1, web-3)
  or it runs a deterministically-better path (web-2). What's
  non-deterministic is WHICH path the run takes, not how each path proceeds.

## Cross-target CPU oracle comparison
- Native CPU oracle output vs web CPU oracle output: **NOT directly
  compared** in this dispatch (web oracle dumps were not serialized to disk
  to avoid log explosion). However, indirect comparison via the
  `oracle=[...]` columns in the per-chunk diff lines shows EVERY web run's
  oracle column at chunk_pos (1,1,1) prints `[31,14,31,3,31,18]` — IDENTICAL
  to native's GPU column (the GPU on native produces the same value the
  oracle does, by definition). The oracle therefore produces the SAME values
  on both targets. The CPU input (the .vox-derived chunk
  Empty/Mixed/Uniform classification, sourced from the GPU's W5 generator
  output via the chunks_cpu lower-u32) does not differ between targets in a
  way that affects oracle output.
- **Different bug class implication:** "CPU oracle differs across targets"
  is REFUTED. The bug is NOT in the .vox load path or the W5 generator
  producing different voxel data on web vs native; it's specifically in the
  W3 regime-2 background loop's chunks AADF EXPANSION on web.

## Native GPU vs Web GPU
- Native GPU produces fully expanded multi-axis AADFs matching the oracle.
- Web GPU on FAIL runs (web-1, web-3) produces `[0,0,0,0,0,0]` for the same
  chunks — meaning **the chunks AADFs are not getting expanded AT ALL** by
  the regime-2 loop on those runs.
- Web GPU on the "lucky" run (web-2) produces `[0,0,15,3,14,14]` —
  expansion happened for some axes (Y±, Z± reached non-zero) but the X axis
  remained at 0. This matches the `IMPLEMENTORS_SHARED.md:413-417`
  observation that the chunks AADF probe at chunk (242,31,219) read
  `chunk_aadf=[0,0,0,0,1,1]` on FAIL runs — only some axes get touched, and
  with the smallest possible expansion (1).

## Verdict
**On wasm32/WebGPU the CPU oracle does NOT produce bit-identical chunks to
the GPU build.** On the 2/3 FAIL runs the GPU's chunk AADFs are 4.147%
identical to the oracle (interior 4.503%) — the GPU values are
`[0,0,0,0,0,0]` for every chunk that should have a non-zero AADF, meaning
the W3 regime-2 expansion is not propagating to the chunks buffer at all on
those runs. On the 1/3 PASS run the GPU values are partially expanded
(`[0,0,15,3,14,14]`-type values, two axes propagated to ~15) and the ratio
climbs to 36.786%, which is enough for the rendered output to clear the SSIM
floor at 0.91 — but still far from the native 100% baseline. **The chunks
buffer carries the wrongness that produces the truncated visual on web; the
bug is in the W3 chunks-AADF pipeline, not downstream raymarcher consumption
nor the .vox load path.**

The diagnostic distinguishes the three possible verdicts the brief
enumerated:
- **GPU on web matches CPU oracle on web** — REFUTED. Ratio is 4-37%, not
  100%.
- **GPU on web differs from CPU oracle on web** — CONFIRMED. The delta
  pattern is "GPU chunks remain at the post-W5-pre-W3-expansion state on
  FAIL runs (all-zero AADFs), partially-expanded on PASS runs". This is
  consistent with the synthesis's C1/C4 (cross-workgroup cross-pass storage
  visibility / RMW hazard on the `chunks[]` buffer) and C3/C6 (cross-encoder
  fence missing / Dawn flush threshold). The bimodal outcome (web-1+web-3
  vs web-2) is consistent with the "lucky path" interpretation — the
  algorithm reaches the second axis's expansion only when a global flush
  happens to fire mid-W3.
- **CPU oracle on web differs from CPU oracle on native** — REFUTED. Same
  oracle values produced at every probed chunk position across both targets.

### Predicted-vs-actual outcome
- **Predicted:** ratio<100% on web; bimodal between unlucky (low ratio) and
  lucky (higher ratio) runs.
- **Actual:** ratio=4.147% on 2/3 unlucky runs (web-1, web-3), 36.786% on
  the lucky 1/3 (web-2). Bimodal as predicted. The "unlucky" ratio is
  startlingly low — the GPU produces ALL-ZERO AADF distances for the
  overwhelming majority of empty chunks, not just degraded values. The PASS
  run still leaves 63% of AADF bytes wrong; whatever interpretation of "the
  algorithm CAN succeed sometimes" the synthesis carried, that
  interpretation has to absorb that even the PASS run is far from oracle.
- Prediction confirmed. The signal is sharper than predicted: on FAIL runs
  the GPU did essentially nothing post-W5 to the chunks buffer's AADF bits,
  not "partial expansion".

## Anomalies observed (raw, no diagnosis)
- The PASS run (web-2) had ratio=36.786% — substantially above the FAIL-run
  baseline of 4.147%, but very far from oracle-matching. The SSIM gate
  threshold of 0.91 is therefore much more permissive than "GPU chunks
  match CPU oracle". A web run could fail the bit-parity diagnostic at any
  ratio between 4% and ~37% and still potentially pass the SSIM gate. This
  is information about what "passing" actually means — not "bit-correct
  chunks", but "enough AADF expansion that the visible truncation moves
  past where the SSIM gate weights the comparison".
- Web-1 and web-3 are **byte-identical** in the diff output — same
  same_bytes count, same first-10 diff slots, same per-slot GPU values.
  Two distinct Playwright runs producing IDENTICAL bit patterns is strong
  evidence the broken-regime-2 trajectory IS algorithmic-deterministic
  (per `04-probe1-impl.md`'s probe-1B verdict). The "non-determinism" is
  in WHICH trajectory the run lands on (low or high), not WHICH bit values
  the trajectory produces.
- Web's `cpu-gpu-parity-diff` rows for the first 10 (boundary) diffs at
  chunk_pos (0..9, 0, 0) all show `gpu=[0,0,0,0,0,0]` on web-1/web-3 and
  `gpu=[0,0,15,3,14,14]` on web-2. The `oracle=[31,15,31,4,31,19]`-type
  values come from the CPU oracle running over the SAME chunk-emptiness
  pattern the web GPU sees — meaning the chunk-CLASSIFICATION half of the
  pipeline (W5 generator → chunk_calc producing Empty/Mixed/Full state
  bits) is identical across targets. The error is ENTIRELY in the
  W3 regime-2 AADF EXPANSION step that writes the lower 30 bits of each
  empty chunk word.
- One web-2 oracle value `[31,14,31,3,31,18]` exactly matches the value the
  GPU produces ON NATIVE for the same chunk. So oracle output is target-
  independent and matches native GPU output — confirming the oracle is
  computing the C# / paper §3.3 reference algorithm correctly.
- The native diagnostic dump produced 8.4 MB binary files for both CPU
  oracle chunks (`native-cpu-chunks.bin`) and GPU chunks (`native-gpu-
  chunks.bin`). `cmp` returns equality between them on both runs and
  cross-run.

## Artifacts on disk
- `target/diagnostics/cpu-gpu-parity/native-cpu-chunks.bin` (CPU oracle
  output, run 2 — last write wins; identical to run 1 by `cmp`)
- `target/diagnostics/cpu-gpu-parity/native-gpu-chunks.bin` (GPU readback,
  run 2 — last write wins; identical to run 1 by `cmp`)
- `target/diagnostics/cpu-gpu-parity/native-run-2-cpu-chunks.bin` (run 2
  snapshot, stashed for cross-run diff)
- `target/diagnostics/cpu-gpu-parity/native-run-2-gpu-chunks.bin` (run 2
  snapshot, stashed for cross-run diff)
- `target/diagnostics/cpu-gpu-parity/native-run-1.log`
- `target/diagnostics/cpu-gpu-parity/native-run-2.log`
- `target/diagnostics/cpu-gpu-parity/web-run-1.log` (Playwright stdout +
  `[wasm-diag]`-prefixed wgpu console lines)
- `target/diagnostics/cpu-gpu-parity/web-run-2.log` (the PASS run)
- `target/diagnostics/cpu-gpu-parity/web-run-3.log`

## Next-dispatch implications (out of scope for this read-only dispatch)

The diagnostic gives a precise measurement that future fix attempts can
target deterministically:
- A successful fix should produce **`ratio=100%` on 3/3 web runs at
  PARITY_TRIGGER_FRAMES=60**, not just an SSIM bump.
- The SSIM gate at 0.91 is permissive enough to admit a 37%-correct chunks
  buffer (web-2). Any prospective fix that passes SSIM 3/3 but stays below
  ratio=100% has not solved the underlying bug — it has only moved the
  visible truncation past the SSIM-relevant frame region.
- The bimodal "all-zero" vs "partial-expansion" GPU output suggests the
  fix needs to address whatever determines which trajectory the run takes
  (per `09-synthesis-classifications.md` §E, candidates are cache state,
  Dawn flush threshold timing, JS thread load, or driver scheduler
  alignment). A fence-based fix (C3/C6 family) would make the "lucky"
  trajectory happen every run; an atomicising-chunks fix (C1/C4 family)
  would make even the "lucky" trajectory produce a higher ratio (and ideally
  100%).
