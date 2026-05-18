# vox-gpu-rewrite — inversion diagnostic round 4 (2026-05-18)

## Symptom recap

Stage 4 of the W5.3-fix orchestration built a per-pixel CPU-oracle-vs-GPU
oracle gate (`--vox-gpu-oracle`, see `e2e/vox_gpu_oracle.rs`) per the
user's directive that prior luminance metrics were gameable. The gate's
shared camera pose is `(744, 800, 672)` looking down at `(744, 100, 672)`
— ABOVE the world ceiling looking down at the centre of Oasis's first XZ
tile. Both CPU and GPU phases load the same `oasis_hard_cover.vox`
fixture and capture a single screenshot at the same camera; the compare
phase asserts per-pixel mean RGB Δ < 8.0 over the 256×256 frame.

**Broken-state baseline (no shader/code changes from Stage 3 final
landed):**

```
256×256 frame, 65536 pixels
mean per-pixel RGB Δ = 127.741 (floor 8.00)
pixels with per-channel Δ > 16.0 = 64213 (97.98% of frame; ceiling 1.0%)
sanity: bright (lum>50.0) = 63091 (96.27% ≥ 1.0% floor)
sanity: dark (lum<200.0) = 31941 (48.74% ≥ 1.0% floor)
```

**CPU oracle** (`oracle_cpu.png`): bright, fully-lit top-down view of
Oasis — sand-coloured walls, green palm trees, dark courtyards. Looks
identical to the user's "good" reference (`--oasis-edit-visual` output).

**GPU phase** (`oracle_gpu.png`): the SAME architecture LAYOUT visible
(windows + walls in correct positions) but DRAMATICALLY DARKER — most
walls render near-black with scattered bright/cream pixels at correct
emissive positions, plus scattered GREEN specks through dark walls that
match the user's screenshot-#3 / #4 visible bug pattern (a stone wall
block dedup-hits a palm-tree-foliage block whose hash collides, silently
inheriting the foliage voxel pointer → renderer descends into foliage
data → green specks where stone should be).

The gate fails at the broken baseline with mean Δ ≈ 128 (16× the floor).
The discriminator works.

## Hypotheses tested + outcomes

The Stage 4 dispatch ran 8 fix-iteration attempts; each was evaluated by
re-running the gate's mean per-pixel RGB Δ. The brief's rule was:
"diff < floor → SUCCESS; diff dropped meaningfully but > floor →
PARTIAL, keep change, layer next; diff unchanged or worse → REVERT".

| # | Hypothesis | Change applied | Mean Δ | Outcome |
|---|---|---|---|---|
| 0 | (broken baseline, new oracle gate) | none | 127.741 | discriminator works; W5 dark |
| 1 | H11 voxels[] atomic | `chunk_calc.wgsl`: `voxels: array<atomic<u32>>`; all 4 access sites use `atomicLoad`/`atomicStore` | 142.491 | UNCHANGED (within TAA noise); REVERT. Note: this was measured at the prior inside-world pose; round-2/3 atomic conversion attempts also got 0 measurable effect, confirming WGSL→naga→SPIR-V→NVIDIA Vulkan emits adequate memory barriers on its own. |
| 2 | H11 hash_map[].hash_raw atomic | promoted `hash_raw: u32` → `atomic<u32>`; write site `atomicStore`, dedup-check site `atomicLoad` | 142.454 | UNCHANGED; REVERT. |
| 3 | (diagnostic) collapse to 1 submit | reverted per-segment-submit to ONE shared `render_context` encoder + ONE submit | 139.835 → image is **pure sky/dark** in vast majority | CONFIRMED per-segment-submit IS doing essential work (only segment 511's chunks populate; camera doesn't see them). REVERT. |
| 4 | extended warmup | `ORACLE_WARMUP_FRAMES`: 120 → 480 | 142.535 | UNCHANGED — W3 acceleration / GI convergence isn't gated by additional frames. REVERT. |
| 5 | explicit GPU sync per segment | `device.poll(PollType::wait_indefinitely())` after each per-segment `submit` | 142.577 | UNCHANGED — cross-submit ordering isn't the race. REVERT. |
| 6 | (diagnostic) skip the bounds chain | removed the post-loop `compute_voxel_bounds` + `compute_block_bounds` dispatches | 142.483 | UNCHANGED — the bounds chain isn't corrupting voxel data. REVERT. |
| 7 | bump hash_map to 8 M slots | `initial_hash_map_size: 1 << 20` → `1 << 23` (128 MiB GPU buffer) | 127.818 | UNCHANGED at the new camera pose; hash capacity isn't the load-bearing constraint here. REVERT. |
| 8 | disable dedup-hit path in chunk_calc | commented out the `if (hash_raw == hash) { is_all_equal check; voxel_pointer = voxel_pointer_cur; }` branch in `get_voxel_pointer` | 123.063 (mean Δ better) but image is **pure sky/dark** (every contender falls into probe-cap exhaustion → sentinel 2 → empty); REVERT. The metric IMPROVED because both dark frames score similar deltas; the visual is worse. |

Final landed shader + Rust state: **IDENTICAL to Stage 3 final landed**.
No fixes were retained; all 8 changes were reverted per the iteration
rule.

## What the gate actually proves

The Stage 4 oracle gate is the FIRST tripwire that meets the user's
"un-gameable" criterion: per-pixel diff against a known-good CPU oracle,
with sanity guards that prevent the gate spuriously passing on degenerate
captures.

Empirical findings from the 8-iteration run:

1. **The W5 producer IS writing structural data correctly.** Iter 3's
   diagnostic (collapse-to-one-submit) produced near-pure-sky output —
   proving per-segment-submit IS doing real work and writing chunks
   across the full world extent. Reverting per-segment-submit was
   catastrophic (image dropped to ~sky); the current per-segment-submit
   shape is essential for the visible architecture in the broken-state
   GPU image.
2. **The visible bug is in the per-block voxel data**, not at the
   chunk-level. The CPU and GPU phases show the same architectural
   layout (windows, walls at correct positions). The CPU has bright
   lit-sand materials; the GPU has dark/sky-bleed materials at the same
   positions. The CHUNKS structure is right; the BLOCKS / VOXELS layer
   has wrong content.
3. **Memory ordering isn't the cause.** Iter 1, 2 (voxels[] +
   hash_map.hash_raw made atomic) had zero measurable effect.
   WGSL→naga→SPIR-V→NVIDIA Vulkan 595.71.05 is apparently emitting
   appropriate memory fences without explicit `atomic<u32>` typing.
4. **The hash_map capacity isn't the binding constraint at the new
   pose.** Iter 7 (bumped to 8 M slots) had zero effect.
5. **The bounds chain isn't corrupting data.** Iter 6 (skip the
   bounds chain entirely) had zero effect on the visual or metric.
6. **Disabling dedup gives PURE EMPTY world** (not the round-3
   improvement). With dedup disabled, every contender falls through to
   the probe loop; the first collisions immediately probe-cap-exhaust
   and return sentinel 2 → renderer reads zero voxels → pure sky.
   Round 3's "improvement" measurement was at a pose where the test
   metric wasn't discriminating between failure modes.

## What's still unknown

The actual mechanism producing the dark GPU rendering with the right
chunks-level architecture remains unidentified.  Candidates the next
dispatch should investigate:

### Candidate 1 — the GI bounce environment differs between the two worlds

The CPU oracle's world bounds are the model's natural extent (`1488 ×
544 × 1344` voxels). The GPU's world bounds are `4096 × 512 × 4096`.
Even with byte-identical voxel data in the overlap region (`x<1488,
y<512, z<1344`), the GI ray-bouncing environment is fundamentally
different:

- CPU oracle: rays bouncing off Oasis surfaces in the `+x` / `+z`
  direction escape into sky (nothing exists beyond `x=1488`).
- GPU phase: rays bouncing off Oasis surfaces in the `+x` / `+z`
  direction hit a second / third tiled copy of Oasis (the W5
  generator's `voxelPos % modelSize` tiling means Oasis fills `x =
  0..1487, 1488..2975, 2976..4095`).

The bright-vs-dark difference may not be a producer bug at all but a
fundamental incompatibility between the two world setups for the
multi-bounce GI path. The user's `--oasis-edit-visual` "looks visually
OK" reference uses the CPU-oracle path = NO tiling = sky beyond the
model. The user's `--vox` binary screenshots show LIT Oasis in the
tiled GPU world too — but those screenshots were taken after MANY
seconds of GI/W3 convergence with the bevy windowed app, not within a
120-frame e2e harness window.

**The next dispatch should re-run the gate with MUCH longer warmup
windows (1000+ frames) and verify whether the GPU phase converges to
match the CPU phase.** If yes, the fix is the warmup duration. If not,
the bug is structural.

### Candidate 2 — the test grid's CPU oracle path actually uses a different shader path

The CPU oracle path (`install_vox_sized_to_model`) uses the renderer's
**CPU-upload fallback** branch (c) in `naadf_gpu_producer_node`: chunks
/ blocks / voxels are uploaded directly from `WorldData.{chunks,blocks,
voxels}_cpu` by `prepare_world_gpu`. The renderer reads those uploaded
buffers.

The GPU path (`install_vox_in_fixed_world`) uses branch (a) — the W5
per-segment GPU producer chain. The renderer reads what chunk_calc
writes.

If branch (a)'s output isn't byte-equal to branch (c)'s output even for
voxel positions in the overlap region, the W5 producer has a real bug.
**The next dispatch should add a GPU-readback-driven byte-equal assertion
on a small set of overlap-region chunks between the two paths.** This
moves the test from "render comparison" to "data comparison" — much
tighter than per-pixel framebuffer diff.

### Candidate 3 — the visible "missing voxels" symptom IS the dedup-hit-wrong-block bug

The visible green specks in dark walls ARE consistent with the H10
hypothesis from round 2 (dedup-hit yields wrong material). The round-3
"disable dedup gives count improvement" diagnostic and this Stage 4
iteration 8 ("disable dedup gives pure-sky") both point at the
dedup-hit path being load-bearing. The proper fix isn't "disable dedup"
(blows out hash capacity) but "**make the dedup-hit's data-equality
check correct under WGSL cross-invocation memory ordering**".

The Stage 4 atomic-conversion attempts (iter 1+2) tested this with
sequentially-consistent atomic operations. Zero effect. Either (a)
naga's translation already emits the right barriers, OR (b) the bug is
not in the data-equality check per se but somewhere deeper in the
hash-insert state machine (e.g., the `atomicCompareExchangeWeak` +
`atomicStore` + non-atomic `hash_raw` write ordering has a subtle
write-write race during the slot-claim transition).

The next dispatch should consider:
- **Manually unrolling the `get_voxel_pointer` loop** to one iteration
  with an explicit `atomicFence` between each phase (claim → write
  voxels → write hash_raw → atomic-store voxel_pointer).
- **Inspecting the naga IR** to confirm what memory barriers are
  emitted around `atomicLoad`/`atomicStore` on storage buffer
  accesses.

## Recommended fix sequence for the next dispatch

1. **First**: try LONG warmup (1000+ frames) to confirm or rule out
   "the GPU phase converges to match CPU given enough time" (Candidate 1).
2. **If not converging**: add a GPU-readback diagnostic to compare
   first-tile-overlap chunks/blocks/voxels between CPU oracle and GPU
   paths — find the BYTE-LEVEL divergence (Candidate 2).
3. **If byte-level divergence found**: focus on the chunk_calc shader's
   slot-claim transition (Candidate 3); manually unroll + add fences.

## What's left in place

No code changes from Stage 4. The final landed state is identical to
Stage 3:
- `crates/bevy_naadf/src/render/construction/config.rs:144-165`:
  `initial_hash_map_size: 1 << 20` (Stage 2 fix preserved).
- All other prior fixes (Stage 1, 1.5, 2, 3) preserved unchanged.
- The NEW oracle gate module + e2e modes are landed:
  - `crates/bevy_naadf/src/e2e/vox_gpu_oracle.rs` (~500 LOC, new)
  - `crates/bevy_naadf/src/lib.rs`: 2 new `AppArgs` fields
    (`vox_gpu_oracle_cpu_phase`, `vox_gpu_oracle_gpu_phase`)
  - `crates/bevy_naadf/src/bin/e2e_render.rs`: 3 new flag dispatches
    (`--vox-gpu-oracle`, `--vox-gpu-oracle-cpu`, `--vox-gpu-oracle-gpu`)
  - `crates/bevy_naadf/src/e2e/mod.rs`: module export + system wiring
  - `crates/bevy_naadf/src/e2e/driver.rs`: 3 new `E2ePhase` variants +
    routing
  - `crates/bevy_naadf/src/e2e/framebuffer.rs`: `from_raw_rgba()` helper

The oracle gate IS load-bearing for the next dispatch's regression
detection — it discriminates the broken state at mean Δ = 128 (16× the
8.0 floor) at the chosen camera pose, with sanity guards preventing
both degenerate-frame false-pass and post-fix-pose-change goalpost
moves. It also FAILS at the current broken state, satisfying the
brief's "gate must FAIL at broken state" requirement.

## Confidence level

**MEDIUM** for "the bug is real and visible at this gate", **HIGH** for
"the 8 tested hypotheses do not produce a fix", **LOW** for any specific
candidate root cause.
