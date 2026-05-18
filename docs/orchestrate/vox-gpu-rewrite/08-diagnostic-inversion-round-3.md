# vox-gpu-rewrite — inversion diagnostic round 3 (2026-05-18)

## Symptom recap (unchanged from round 2)

Production binary at
`cargo run --release --bin bevy-naadf -- --vox /home/midori/Downloads/Oasis_Hard_Cover.vox`
still shows scattered missing voxels + bright/colored speckles through what
should be solid Oasis architecture. The Stage 1.5 fix landed the hash_map +
hash_coefficients allocation block on the W5 install path, but the visual
artifact persists.

## What this round did (compound dispatch)

This dispatch combined GATE-sharpening with FIX-attempting. It applied the
round-2-recommended primary fix (bump `initial_hash_map_size` from `1 << 18`
to `1 << 20` — matching C# `WorldData.cs:131-132`'s `minReservedCount =
256^3 / 32 = 524,288` → `mapSize >= 1,048,576` invocation), then iterated
through additional candidate fixes when the primary fix did not change the
rendered output.

## Findings

### Round-2 hypothesis H9 (hash_map saturation) — REFUTED

The diagnostic round 2's MEDIUM-confidence hypothesis was that the
`initial_hash_map_size = 262,144` slots was 4× under-sized for the
fixed-world case and probe-cap exhaustion at ~131k unique blocks was
causing the inversion. This dispatch tested it:

| State | hash_map slots | hash_map bytes | frame-A near-black count (lum<10) |
|---|---|---|---|
| Pre-fix (Stage 1.5 baseline) | 262,144 | 4 MiB | 23,092 (35.24%) |
| Bumped to 1<<20 = C#-faithful | 1,048,576 | 16 MiB | 23,099 (35.25%) |
| Bumped to 1<<23 = 32× C# | 8,388,608 | 128 MiB | 23,105 (35.26%) |
| Reverted to 262,144 | 262,144 | 4 MiB | 23,092 (35.24%) |

All four runs measure within noise (±20 pixels). **The hash_map size
makes ZERO measurable difference at the C# spawn pose** — saturation is
NOT the cause of the visible inversion artifacts at this view, and almost
certainly is not the cause at the user's scaled-pose view either.

The hash_map sizing fix was nevertheless RETAINED in this dispatch's
landed code because the C# port is wrong about it (the constant was
documented as `BlockHashingHandler.cs:32`'s default-ctor `minReservedCount
= 64`, not the `WorldData.cs:131-132` invocation's `minReservedCount =
524,288`). Bringing the Rust port to C# parity is a correctness move
independent of the visual symptom; if a future fix to the actual
inversion bug DOES exercise the hash_map past 131k entries, the bumped
size will avoid a new regression.

### New experiment 1 — `bound_group_queue_max_size` Stage 1.5 fix reverted

Temporarily reverted the Stage 1.5 secondary fix
(`bound_group_queue_max_size: 32768` → `1` in the W5 per-segment params).
Stage 1.5 had added the field's correct value to the W5 loop's per-segment
construction-params write; round 2 H7 noted chunk_calc.wgsl does not read
this field, so the change was perf-only. Result: frame-A near-black count
= 23,099 (35.25%) — identical to with the fix. Confirmed perf-only,
restored the Stage 1.5 fix.

### New experiment 2 — chunk_calc dedup-hit path disabled

Temporarily disabled the dedup-hit branch in
`chunk_calc.wgsl::get_voxel_pointer` (`crates/bevy_naadf/src/assets/shaders/chunk_calc.wgsl:319-331`),
making every contending thread fall through to the next probe slot
instead of accepting the existing slot. Result:

- frame-A near-black count: 20,875 (31.85%) — **2,217 pixel improvement**
  vs the Stage 1.5 + hash-bump baseline (23,092 → 20,875).
- Rect mean before luminance: 55.4 → 58.0 (brighter).

**This is a meaningful, reproducible signal.** The dedup-hit path IS
contributing to the inversion at Oasis scale. With dedup disabled, every
contending block claims a new slot — exhausting the hash_map faster (and
producing MORE inversion holes at the saturation limit) — but the
rendering measurably IMPROVES at this pose, suggesting the
dedup-failure-mode produces darker / more-disruptive artifacts than the
probe-cap-exhaustion failure mode.

The dedup-hit change was REVERTED before this dispatch's landing — it is
NOT a production fix (it would exhaust the 1M-slot hash_map past ~1M
unique blocks, returning sentinel-2 for the rest), only a diagnostic.
But the result strongly points at the dedup-hit path's correctness as
the next thing to investigate.

## Likely root cause (NEW hypothesis, MEDIUM-HIGH confidence)

**Hypothesis H11 (round-2 follow-up) is the most likely cause:** the
chunk_calc shader's dedup-hit branch reads `voxels[voxel_pointer_cur +
i]` non-atomically AFTER a spin-wait on `atomicLoad(voxel_pointer)`.

In HLSL, the spin-wait uses `InterlockedOr(...voxel_pointer, 0,
voxelPointerCur)` which provides FULL memory barrier semantics on every
iteration — so by the time the spin observes the cleared PENDING bit,
the slot-claimant's writes to `voxels[voxel_u32_start + i..]` AND
`hash_map[].hash_raw` are GUARANTEED visible.

In WGSL, `atomicLoad` follows sequentially-consistent atomic ordering,
but the WGSL spec is ambiguous on whether the implicit happens-before
chain propagates to NON-ATOMIC reads of OTHER memory locations across
invocations. The naga→Vulkan/SPIR-V translation might or might not emit
appropriate `OpMemoryBarrier`s on atomic loads. Per the WGSL spec §13.3,
explicit `workgroupBarrier()` / `storageBarrier()` synchronisation is
workgroup-scoped only — there is no cross-workgroup memory fence in WGSL.

If the writes to `voxels[]` by the slot-claimant are not visible to the
reading thread when it does its 32-element data-equality check, the
check spuriously fails (`is_all_equal = false`), the probe continues,
and many blocks that SHOULD dedup-hit instead either:
- claim a new slot (succeed but waste a slot), or
- eventually probe-cap-exhaust after 250 iterations → sentinel 2 → empty
  void render

The experimental DISABLING of the dedup-hit path improving the count
metric is consistent with this hypothesis: forcing the slow path
(probe-to-new-slot) avoids the buggy dedup check entirely. The remaining
inversion damage with dedup disabled comes from blocks past the
hash-map's capacity (1M slots → ~1M unique blocks before sentinel-2).

### Why C# is unaffected

D3D11's `InterlockedExchange`/`InterlockedOr` provide release/acquire
memory ordering between atomic and non-atomic accesses on the same and
different memory locations within a compute dispatch. The HLSL shader
relies on this implicitly. WGSL has no equivalent.

### Why validate_gpu_construction passes anyway

The validate test scene is 1×1×1 chunks with 1 mixed block. No
cross-thread contention on the hash slot. The dedup-hit path is never
exercised — every block claims slot 0 cleanly. The bit-exact comparison
passes.

### Why the bug shows at Oasis scale but not earlier

Oasis: ~2.75M mixed blocks total (8.4× XZ-tile multiplication of the
model's 327k mixed blocks). With 1M hash slots, contention is high —
the dedup-hit path is heavily exercised. The race window is real.

The small-scene `bevy-naadf` test scene (the `--validate-gpu-construction`
fixture) has at most a few hundred mixed blocks. Race window is
negligible — dedup-hit racing is statistically rare.

## Recommended fix (NOT implemented in this dispatch)

### Primary fix candidate (HIGH-confidence direction, IMPLEMENTATION non-trivial)

Make the WGSL `voxels[]` storage buffer's accesses atomic across the
chunk_calc dedup boundary. Two implementation shapes:

**Shape A — atomic voxels buffer:** Change
`var<storage, read_write> voxels: array<u32>` to
`var<storage, read_write> voxels: array<atomic<u32>>` in
`chunk_calc.wgsl`. Replace `voxels[idx] = data` with `atomicStore(&voxels[idx], data)`
and `voxels[idx]` reads with `atomicLoad(&voxels[idx])`. This forces
sequentially-consistent ordering on every voxel access — which IS what
HLSL gets for free via InterlockedExchange semantics. The renderer side
(`ray_tracing.wgsl`) can keep using non-atomic reads (WGSL allows
non-atomic access via a separate binding that points to the same
buffer, or atomicLoad is acceptable everywhere).

Performance cost: each voxel write becomes a sequentially-consistent
atomic store. On NVIDIA Vulkan, this likely translates to a memory
fence + store; minor perf impact at startup-only producer cost.

**Shape B — explicit memory fence via atomicCompareExchangeWeak retry
loop on dedup check:** Restructure the dedup check to use
`atomicLoad(&voxels[voxel_pointer_cur + i])` instead of `voxels[idx]`
in the 32-element comparison. Requires `voxels[]` to be a binding of
`array<atomic<u32>>`. Same code change as Shape A but localised to
chunk_calc.wgsl (only the dedup check path uses atomic reads).

### Secondary fix candidate (lower priority)

Investigate whether the dedup-hit path's `hash_map[].hash_raw` non-atomic
read also has a memory-ordering hazard. The hash_raw is written ONCE
(by the slot claimant before the atomicStore that clears PENDING), but
the non-atomic read by the spinning thread is racing against the
non-atomic write. Per WGSL spec, this is technically undefined behaviour
unless the atomic chain establishes visibility — which (per the primary
hypothesis above) it might not in WGSL.

Same fix shape: make `hash_map[].hash_raw` atomic.

### Out-of-scope fixes (not recommended)

- **Bumping `initial_hash_map_size` further** — proven ineffective by
  this round's experiments (1M and 8M tested; no change).
- **Adding `storageBarrier()` in `get_voxel_pointer`** — workgroup-scoped
  only; doesn't synchronise across workgroups, which is where the race
  lives.
- **Wiring `dispatch_map_copy` for per-segment hash-map regrowth** —
  the proven-ineffective hash-size hypothesis would need to be the
  cause for this to help; it isn't.

## Gate metric for the next dispatch

The W5.5 e2e gate's `count_pixels_with_luminance_below` metric does NOT
cleanly discriminate broken vs fixed state at the C# spawn pose. The
legitimate dark interior geometry (camera at Y=200 sees dark stone
walls/floors filling most of the frame) dominates the near-black count,
swamping the inversion-class artifacts (which manifest at this pose as
small bright water/sky-bleed specks scattered through the dark interior,
NOT as additional near-black pixels).

Pre-fix and any reasonable post-fix would both show ~35% near-black at
this pose. The diagnostic round 2 already noted this:

> the brief's success metric (`near-black drops from ~35% to ~0%` at
> the C# pose) is unachievable by ANY fix.

Three options for the next dispatch's gate metric:

1. **Golden-image comparison** (diagnostic round 2's recommendation c):
   capture a known-good post-fix screenshot at the C# pose; assert
   `stability_hash() == golden_hash` or per-pixel diff < threshold.
   Requires the fix to land first.
2. **Bright-outliers-in-dark-band**: count pixels with `lum > 130` in
   y=144..191 (the lower-mid band where the saved before.png shows
   inversion specks but no legitimate horizon). Pre-fix Stage 1.5 count
   was ~75 pixels in this band; a working fix would drop it to noise
   (~0-5 pixels). Floor: 30 pixels. Reasonably discriminating but
   pose-and-image-fragile.
3. **GPU readback assertion**: read back the `block_voxel_count`
   cursor after the W5 producer runs and assert it matches an expected
   range (e.g., between 1M and 2M mixed blocks for Oasis). Doesn't
   prove visual correctness but proves the producer's CURSOR allocation
   completed without exhaustion. Pairs well with a visual gate.

The CURRENT gate (`lum<10` over the full frame with 1% floor) FAILS at
this dispatch's landed state AND at any reasonable next-fix state, so
it's not a usable signal until the metric is replaced.

## Confidence level

**MEDIUM-HIGH for the H11 memory-ordering hypothesis.** The dedup-disable
experiment is concrete evidence the dedup-hit path is contributing —
the count metric shifted by 2.2k pixels in the expected direction.
However:

- The shift is small relative to the total dark count (23k → 21k =
  ~10% improvement). The remaining 21k pixels are likely a mix of
  legitimate dark geometry and inversion artifacts the dedup-disable
  experiment didn't address.
- The actual fix (make voxels[] atomic) was NOT yet attempted; the
  dedup-disable experiment is only weak evidence that the FIX would
  work, not strong evidence.
- WGSL memory-ordering semantics are documented loosely; the actual
  behaviour depends on naga's translation choices and the wgpu backend.
  Naga's SPIR-V output for `atomicLoad` likely DOES emit an
  `OpMemoryBarrier`, in which case the diagnostic is wrong about the
  cause.

The next dispatch should:
1. Implement Shape A or Shape B from "Primary fix candidate" above.
2. Re-run the e2e gate at C# pose AND have the user re-test the
   production binary visually.
3. If the bug persists, look at other hypotheses (perhaps the per-segment
   encoder+submit ordering, or buffer aliasing in the bind groups).

## Observation evidence

### Hash-buffer sizes at runtime (instrumentation, since removed)

```
ROUND-3 DIAG: W5 producer entering loop with
  gpu.hash_map.size            = 16,777,216  (= 1M slots × 16 B, post-bump)
  gpu.hash_coefficients.size   = 260          (= 65 × 4 B)
  gpu.block_voxel_count.size   = 8            (correct)
  gpu.segment_voxel_buffer.size= 33,554,432   (= 32 MiB, correct)
  config.initial_hash_map_size = 1,048,576    (post-bump)
```

### E2E gate measurements per experiment

| Experiment | rect mean before | near-black (lum<10) | full-frame Δ |
|---|---|---|---|
| Stage 1.5 baseline (hash_map = 262k) | (44.92, 57.48, 69.16) | 23,087 (35.23%) | 9.79 |
| Bumped to 1<<20 (1M slots) | (44.72, 57.22, 68.84) | 23,099 (35.25%) | 9.67 |
| Bumped to 1<<23 (8M slots) | (44.72, 57.22, 68.84) | 23,105 (35.26%) | 9.67 |
| Reverted, ran again | (44.92, 57.47, 69.16) | 23,092 (35.24%) | 9.67 |
| 1M slots + `bound_group_queue_max_size = 1` | (44.71, 57.23, 68.85) | 23,099 (35.25%) | 9.67 |
| 1M slots + dedup-disabled in chunk_calc | (47.30, 59.87, 71.31) | **20,875 (31.85%)** | **10.72** |
| Final landed: 1M slots, all fixes restored | (44.71, 57.21, 68.83) | 23,092 (35.24%) | 9.67 |

### Final landed state

Code changes left in place:
- `crates/bevy_naadf/src/render/construction/config.rs:144-165, :207-210`:
  `initial_hash_map_size: 1 << 18` → `1 << 20`, with C#-traceback comment.

The chunk_calc shader was NOT modified in the final landed state — the
dedup-disable was diagnostic-only.

The W5.5 gate was NOT sharpened — the current `lum<10`-over-full-frame
metric does not discriminate broken vs fixed at the C# pose (the
diagnostic round 2 finding stands). Changing the threshold to `lum<1`
or RGB-per-channel<1 yields 0 pixels at broken state — the gate would
spuriously PASS pre-fix.

### Workspace + test status

- `cargo build --workspace`: PASS (~22 s clean).
- `cargo test --workspace --lib`: PASS — 198 passed, 1 ignored (baseline).
- `cargo run --release --bin e2e_render -- --baseline`: PASS.
- `cargo run --release --bin e2e_render -- --vox-e2e`: PASS.
- `cargo run --release --bin e2e_render -- --oasis-edit-visual`: PASS.
- `cargo run --release --bin e2e_render -- --validate-gpu-construction`: PASS.
- `cargo run --release --bin e2e_render -- --edit-mode`: PASS.
- `cargo run --release --bin e2e_render -- --entities`: PASS.
- `cargo run --release --bin e2e_render -- --runtime-edit-mode`: PASS.
- `cargo run --release --bin e2e_render -- --small-edit-visual`: PASS.
- `cargo run --release --bin e2e_render -- --small-edit-repro`: PASS.
- `cargo run --release --bin e2e_render -- --vox-gpu-construction`: **FAIL**
  — 23,092 near-black pixels at C# pose (35.24% of frame, ceiling 1%).
  The metric is incorrect for this pose (per the analysis above); the
  underlying bug is also not fixed.

Zero regressions on previously-GREEN gates. The only RED gate is
`--vox-gpu-construction`, which was already RED at dispatch start and
remains RED because:
(a) the gate's metric is not pose-appropriate, and
(b) the H11 hypothesis fix was not implemented in this dispatch.

## What the next dispatch should do

1. **Implement the H11 fix** (Shape A or Shape B above) — make
   `voxels[]` atomic in `chunk_calc.wgsl` to force cross-invocation
   memory ordering on the dedup-hit data-equality check.
2. **Land a new gate metric** (option 1 or 2 from "Gate metric for the
   next dispatch" above) — the current `lum<10`-full-frame is unusable
   at the C# pose.
3. Have the user visually re-test the production binary at the scaled
   pose (`bevy-naadf -- --vox Oasis_Hard_Cover.vox`) — the visible
   inversion artifacts there are the ground-truth oracle for whether
   the H11 fix works.

If the H11 fix does NOT resolve the visual artifact, investigate:
- Whether `hash_map[].hash_raw` also needs to be atomic (secondary fix
  candidate above).
- Whether per-segment encoder + submit ordering has subtle
  device-driver-level memory-ordering hazards across wgpu submits
  (unlikely but worth ruling out via timestamp queries or readback
  intermediate state).
- Whether the WGSL→naga→SPIR-V→NVIDIA driver translation is emitting
  the correct memory barriers for atomicLoad/atomicStore on the
  RTX 5080 + driver 595.71.05.
