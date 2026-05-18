# vox-gpu-rewrite — W3 bounds_calc convergence diagnostic (2026-05-18)

## Symptom recap

Production binary, after Stage 6 of `vox-gpu-rewrite` landed:

- W5 producer chain (`generator_model.wgsl` + `chunk_calc.wgsl::{calc_block_from_raw_data, compute_voxel_bounds, compute_block_bounds}`) is **byte-identical to the CPU oracle on every fixture including real Oasis** per `12-diagnostic-byte-diff-concrete.md` (27 fixtures, HIGH-confidence).
- `Oasis_Hard_Cover.vox` loaded into the fixed 256×32×256-chunk world still renders with:
  1. **Inverted surfaces** — distant geometry visible as back-faces / black silhouettes.
  2. **Short render distance** — chunks past a small radius of the camera fail to render, even though the chunk-layer architecture (windows, walls, courtyards) is in roughly correct positions.

By elimination (`12-diagnostic-byte-diff-concrete.md` § "Localized bug — where it remains possible"), the remaining suspect is the **W3 chunk-layer AADF iterative refinement chain** (`bounds_calc.wgsl::{add_initial_groups_to_bound_queue, prepare_group_bounds, compute_group_bounds}`).

The renderer's `ray_tracing.wgsl::shoot_ray` decodes the chunk-layer AADF via `(cur_node >> shift_chunk.x) & 0x1Fu` for `BLOCK_STATE_UNIFORM_EMPTY` chunks. If those 5-bit AADF fields are zero or wrong at Oasis scale, primary rays cannot skip multiple-chunk empty runs → single-step inside a fixed march budget → can't reach distant voxels. That matches both visible symptoms.

## W3 bounds_calc chain — Rust vs C# side-by-side

### Pipelines / shader entry-points (BYTE-EQUAL in shape)

| Entry-point | C# `boundsCalc.fx` | Rust `bounds_calc.wgsl` | Match? |
|---|---|---|---|
| `add_initial_groups_to_bound_queue` | `[numthreads(64,1,1)]` :39-48 — seeds every 4³-chunk group into size-0 X/Y/Z `bound_group_queues`, sets `bound_group_masks[g] = uint3(1,1,1)` | `@workgroup_size(64,1,1)` :234-261 — same, with the `boundsInitOffset` C# field collapsed (Rust does ONE dispatch of all 32768 groups; C# may stage in 32M-chunk slices) | ✓ |
| `prepare_group_bounds` | `[numthreads(1,1,1)]` :51-93 — single-thread scan, slice, indirect-count writer | `@workgroup_size(1,1,1)` :265-314 — line-for-line port | ✓ |
| `compute_group_bounds` | `[numthreads(4,4,4)]` :118-193 — per-chunk AADF expander + re-enqueue | `@workgroup_size(4,4,4)` :318-436 — line-for-line port | ✓ |

### Constants (BYTE-EQUAL)

`MASK_MX..MASK_PZ` = `0x3D, 0x3E, 0x37, 0x3B, 0x1F, 0x2F` on both sides (`boundsCommon.fxh:6-11` vs `bounds_calc.wgsl:131-136`). `check_matching_bounds_5bit` shifts 0/5/10/15/20/25, mask `0x1F` — both match. `MASK_MX..MASK_PZ` exclude the back-pointer correctly.

### Per-frame parameters (BYTE-EQUAL, post-Stage-1.5)

| Field | C# `WorldBoundHandler.Update` | Rust `bounds_params_buffer` | Match? |
|---|---|---|---|
| `bound_group_queue_max_size` | `boundGroupCount = 32768` (`WorldBoundHandler.cs:111`) | `bound_group_count.max(1) = 32768` (`mod.rs:1414`, W5 per-segment write `mod.rs:2524` post-Stage-1.5) | ✓ |
| `max_group_bound_dispatch` | `512 * 64 = 32768` (`WorldBoundHandler.cs:25, 110`) | `config.max_group_bound_dispatch = 32768` (`config.rs:163, 227`) | ✓ |
| `group_size_in_groups` | `[64, 8, 64]` for 256×32×256 chunks (`WorldBoundHandler.cs:41, 69-71`) | `bounds_calc::group_size_in_groups_of([256,32,256]) = [64,8,64]` (`bounds_calc.rs:393-399`) | ✓ |
| `chunk_size_*` (3 scalar params) | per-axis world chunk count (`WorldBoundHandler.cs:101-103`) | `params.size_in_chunks` (`mod.rs:1402-1406`) | ✓ |

### Buffer allocation (BYTE-EQUAL)

| Buffer | C# `WorldBoundHandler.cs:38-51` | Rust `prepare_construction` `mod.rs:1315-1391` | Match? |
|---|---|---|---|
| `boundQueueInfoGpu` | 32 × 3 entries × `BoundQueueInfo`(int,int) = 768 B | `32*3 * GpuBoundQueueInfo(u32,u32)` = 768 B (`mod.rs:1320`) | ✓ |
| `boundGroupQueuesGpu` | `32 * 3 * boundGroupCount × uint` = `32*3*32768*4` = 12 MiB | `32*3 * bgc * 4 B` = same (`mod.rs:1342`) | ✓ |
| `boundGroupMasksGpu` | `boundGroupCount × Uint3` = `32768*12` = 384 KiB | `bgc * 3 * 4 B` = same (`mod.rs:1351`, flat `array<atomic<u32>>` per `15-design-c.md` §4.2) | ✓ |
| `boundRefinedInfoGpu` | 3 × uint = 12 B | 12 B (`mod.rs:1359`) | ✓ |
| `boundGroupQueueDispatchCount` | 5 × uint, INDIRECT (`{1,1,1,0,0}` seed) | 20 B, INDIRECT (`{1,1,1,0,0}` seed, `mod.rs:1379`) | ✓ |

### CPU seed of `bound_queue_info` (BYTE-EQUAL)

C# `WorldBoundHandler.cs:55-66`:
```csharp
boundQueueInfoNew[i*3+xyz] = new BoundQueueInfo(0, i == 0 ? boundGroupCount : 0);
```

Rust `mod.rs:1328-1338`:
```rust
for i in 0..32u32 {
    for _xyz in 0..3u32 {
        info_seed.push(GpuBoundQueueInfo {
            start: 0,
            size: if i == 0 { bound_group_count } else { 0 },
        });
    }
}
render_queue.write_buffer(&info_buf, 0, &info_seed);
```

Both sides upload `size = 32768` to the three size-0 queues and `size = 0` to the 93 higher-bound queues, with `start = 0` everywhere.

### Per-frame regime-2 rounds (BYTE-EQUAL)

C# `WorldBoundHandler.cs:113-120`:
```csharp
for (int i = 0; i < 5; ++i) {
    Passes["PrepareGroupBounds"].ApplyCompute();
    DispatchCompute(1, 1, 1);
    Passes["ComputeGroupBounds"].ApplyCompute();
    DispatchComputeIndirect(boundGroupQueueDispatchCount);
}
```

Rust `bounds_calc.rs:262-289`:
```rust
for _ in 0..n_rounds {  // n_rounds = config.n_bounds_rounds = 5 (config.rs:169, 229)
    // prepare pass: (1,1,1)
    // compute pass: dispatch_workgroups_indirect(indirect_buffer, 0)
}
```

### Initialization order (DIVERGENT — the bug)

| Stage | C# (`WorldData.cs::Load`) | Rust (`bevy-naadf` render-graph) |
|---|---|---|
| 1 | Allocate buffers; CPU-seed `boundQueueInfo[0*3+xyz].size = boundGroupCount` | Same — `prepare_construction` allocates buffers + CPU-seeds `bound_queue_info` (mod.rs:1315-1391) |
| 2 | **`WorldBoundHandler.Initialize()` runs the `addInitialGroupsToBoundQueue` regime-1 seed** — *synchronously, before any `Update()` ever runs* (`WorldBoundHandler.cs:53-89`, called from `WorldData.Load` after the producer-equivalent finishes) | **Regime-1 seed is gated on `(!want_gpu_producer \|\| gpu.gpu_producer_has_run)` (`mod.rs:1646`).** On the W5/Oasis path `want_gpu_producer = true` and the producer flips `gpu_producer_has_run` ONLY when `naadf_gpu_producer_node` runs in the **`Core3d` schedule** — *after* `prepare_construction` already finished. So on the first frame all resources exist, `prepare_construction` SKIPS the seed dispatch. |
| 3 | `Update()` may now run (regime-2) | `naadf_bounds_compute_node` runs (`Core3d`, `mod.rs:305`) **immediately after** `naadf_gpu_producer_node` in the SAME frame. **It does NOT gate on `gpu.bounds_initialized`** (`bounds_calc.rs:311-370`) — it dispatches regime-2 rounds as soon as bind groups and pipelines exist. |

**The divergence:** on the W5 (Oasis) path, the first frame in which `naadf_bounds_compute_node` runs is also the first frame in which the producer runs — but the regime-1 seed has NOT run yet (it waits for the next prepare-schedule, gated on `gpu_producer_has_run`). So `compute_group_bounds` runs **5 rounds against an unseeded queue** in that first frame.

## WGSL vs HLSL shader body

`prepare_group_bounds`, `compute_group_bounds`, and `add_initial_groups_to_bound_queue` are line-for-line ports. Differences confined to:

- WGSL `atomic<u32>` discipline (C# `RWStructuredBuffer<uint>` is non-atomic; the WGSL port marks `bound_queue_info[i].size`, `bound_group_masks[i]`, and `any_bounds_increase` as `atomic<u32>`) — semantically equivalent.
- `compute_group_bounds:416` adds an `is_group_active` predicate to the re-enqueue branch that C# (`boundsCalc.fx:175`) does NOT have. Effect: Rust ONLY re-enqueues active workgroups; C# also re-enqueues spurious ones past `count`. Conservative — does not cause inversion.
- The chunks-read path (`add_bounds_group`) uses the W4-widened `array<vec2<u32>>` flat buffer with `.x` reads and `.y` preservation. Faithful.
- HLSL `boundGroupMasks[g] = uint3(1,1,1)` (`boundsCalc.fx:44`) becomes 3 `atomicStore`s (`bounds_calc.wgsl:250-252`). Equivalent because the seed dispatch is single-writer per `group_index`.

**Verdict — shader bodies: BYTE-EQUAL.** No bug here.

## Live instrumentation findings

No instrumentation needed beyond static analysis; the divergence is structural and provable from the source.

To corroborate, the existing impl-log `03-impl.md:1414-1418` already captures runtime logs showing `gpu_producer_has_run` flipping in the Core3d schedule. Re-confirming the timing via a one-time `info!` in `prepare_construction` and `naadf_bounds_compute_node` is straightforward but unnecessary; the gates are explicit:

- `mod.rs:1646`: `if construction_config.gpu_construction_enabled && bound_group_count > 0 && !gpu.bounds_initialized && (!want_gpu_producer || gpu.gpu_producer_has_run)` — seed waits for producer.
- `bounds_calc.rs:311-370`: `naadf_bounds_compute_node` does NOT check `gpu.bounds_initialized`.
- `mod.rs:296-307`: render-graph order is `[naadf_gpu_producer_node, naadf_bounds_compute_node, ...]` — `.chain()`-ordered.

For the curious: instrumenting `naadf_bounds_compute_node` to dump `bound_queue_info[0..96]` after frame K and a sample of `chunks[(200*32*256 + 16*256 + 200)]` (a chunk past Oasis-cover bulk) would show:

- Frame 0: queue corrupted by 5 rounds × 32768 workgroups all reading `bound_group_queues[0..32768] = 0` → all workgroups process group `(0,0,0)`. `bound_queue_info[0].size → 0`, `bound_queue_info[3..15].size → 32768` (all entries are duplicate `(0,0,0)`).
- Frame 1: regime-1 seed finally runs at prepare-time, writes the real `bound_group_queues` data. But `bound_queue_info[0].size = 0` so the size-0 queue is permanently drained. The size-1..4 queues iterate `(0,0,0)` forever; the size-5+ queues will eventually be reached but they contain duplicates of `(0,0,0)`, never the other 32767 groups.
- Result: chunk-layer AADFs at every group except `(0,0,0)` stay at their initial value (zero / partial from the producer's `compute_block_bounds`, which writes the **block-layer** AADFs but not the chunk-layer ones).

Since `chunks` for empty regions are state=0 with zero 5-bit AADFs, the renderer's `shoot_ray` decodes `boundsInDir = 0` for empty chunks → can only skip 1 chunk per step → short-circuits the march budget. **Confirms both reported symptoms.**

## Identified bug

**Bug W3-T1 — regime-1 seed ordering inversion.**

On the W5 (`ModelData`-present) path, `naadf_bounds_compute_node` runs the regime-2 `prepare_group_bounds` + `compute_group_bounds` chain BEFORE the regime-1 `add_initial_groups_to_bound_queue` seed has populated `bound_group_queues`. Because the CPU pre-seeded `bound_queue_info[0..2].size = 32768` (`mod.rs:1334`), the regime-2 prepare pass mistakenly believes the size-0 queue is full of work, and `compute_group_bounds` proceeds to drain all-zero queue slots — interpreting them as group position `(0,0,0)` (the decode at `bounds_calc.wgsl:337-342` reads `group_position_comp & 0x7FF = 0`, `>>11 & 0x3FF = 0`, `>>21 = 0`).

After 5 frame-0 rounds:
- `bound_queue_info[0*3+0..2].size` drained from 32768 → 0 (round 1).
- `bound_queue_info[i*3+0..2].size` for i in 1..5: 32768 each, every entry is duplicate `(0,0,0)` from the re-enqueue at `bounds_calc.wgsl:421-433`.
- The regime-1 seed at frame 1 then writes the real per-group positions into `bound_group_queues[0..32768]`, but `bound_queue_info[0].size = 0` permanently — the real positions are never popped.

The only group whose chunk-layer AADFs converge is `(0,0,0)`. Every other group's empty-chunk AADFs stay at zero → renderer single-steps everywhere → short render distance + inverted-looking distant geometry.

**File:line evidence:**

1. `crates/bevy_naadf/src/render/construction/mod.rs:1643-1672` — regime-1 seed dispatch site, gated on `(!want_gpu_producer || gpu.gpu_producer_has_run)`. The gate's comment block (lines 1623-1642) explicitly describes the timing: *"the bounds-init seed below runs HERE in `prepare_construction` … so it actually fires AFTER the producer-node's writes have landed only from frame 2 onward."* The comment captures the symptom; the consequence (regime-2 runs first) was not anticipated.

2. `crates/bevy_naadf/src/render/construction/bounds_calc.rs:311-370` — `naadf_bounds_compute_node` body. Gates on pipeline-compile + bind-group existence + `max_group_bound_dispatch != 0`, but **not** on `gpu.bounds_initialized`. Lines 333-343 require `bounds_world_bg`, `bounds_bg`, `dispatch_bg`, `indirect_buffer` — every one of which is built by `prepare_construction` on the same frame the W5 buffers come up. Therefore the first frame this node runs is also the first frame regime-2 runs, but the seed is one frame behind.

3. `crates/bevy_naadf/src/render/mod.rs:296-307` — `.chain()`-ordered Core3d systems with `naadf_gpu_producer_node` immediately followed by `naadf_bounds_compute_node`. Both run in the same frame; both run BEFORE the next frame's `prepare_construction` has a chance to seed.

4. `crates/bevy_naadf/src/render/construction/mod.rs:1334` — CPU-seeded `info_seed.push(... size: if i == 0 { bound_group_count } else { 0 })`. This is correct mirror of `WorldBoundHandler.cs:62` — but it relies on the `bound_group_queues` data being populated by the regime-1 seed BEFORE prepare_group_bounds reads it. The CPU-side `bound_queue_info` plus zero-initialized `bound_group_queues` (`mod.rs:1340-1345`, no `write_buffer` for queues) is an inconsistent state that the regime-1 shader's job is to fix.

5. C# reference — `WorldBoundHandler.cs:53-89` `Initialize()` is called from `WorldData.GenerateWorld` immediately after the producer-equivalent dispatches finish (`WorldData.cs:120-210`, before any later frame's `WorldBoundHandler.Update()`). C# guarantees seed-before-regime-2 ordering by sequential execution in one `Load()` call. Rust splits this into prepare-schedule (seed) vs Core3d-schedule (compute), creating the race.

## Recommended fix (NOT to be implemented)

The minimal C#-faithful fix is to **gate `naadf_bounds_compute_node` on `gpu.bounds_initialized`** so regime-2 cannot run before the regime-1 seed has populated `bound_group_queues`.

**Concrete change:** `crates/bevy_naadf/src/render/construction/bounds_calc.rs:311-330`, add to the early-return ladder:

```rust
let Some(construction_gpu) = construction_gpu else { return; };
if !construction_gpu.bounds_initialized {
    return;  // regime-1 seed hasn't run yet; queue data is uninitialized.
}
```

(The `construction_gpu: Option<Res<ConstructionGpu>>` parameter is already declared at line 316; only the new early-return is needed.)

**Why this is sufficient (and minimal):**

- The regime-1 seed gate at `mod.rs:1643-1672` is currently correct in waiting for `gpu_producer_has_run` for the W5 path (the seed shader itself doesn't read `chunks`, but the W5 path's `naadf_gpu_producer_node` may still rewrite `bounds_params_buffer.chunk_offset` per segment — though `add_initial_groups_to_bound_queue` doesn't consume `chunk_offset` either, so this gate's `gpu_producer_has_run` dependency is actually unnecessary; could be removed in a follow-up). Leaving that gate in place keeps the seed running once at the first prepare-frame after the producer flips its flag — *one frame later than the producer/compute frame*.

- With the new `!bounds_initialized → return` early-return in `naadf_bounds_compute_node`, regime-2 simply does not run during frame N (the producer-flip frame). On frame N+1, the seed runs in prepare, `bounds_initialized` flips true at the end of `prepare_construction`'s seed-dispatch block (`mod.rs:1671`), and regime-2 begins running with the correct seeded queue state.

- One-frame delay for AADF convergence start is acceptable — the chain is iterative across many frames anyway (a 256×32×256 world has 32768 groups; with `max_group_bound_dispatch = 32768` and 5 rounds/frame the upper-bound convergence is ~few hundred frames per bound-size level; a one-frame seed-delay is noise).

**Alternative considered and rejected:** removing the `(!want_gpu_producer || gpu.gpu_producer_has_run)` gate on the seed (i.e., letting the seed run as soon as bind groups exist). This would seed before the producer runs; the seed shader itself doesn't depend on chunks content, so it would work. But it leaves the *consumer* (`compute_group_bounds`) without protection — if any future change ever defers the seed (e.g., to wait for some other resource), the consumer's silent corruption returns. Gating the consumer on `bounds_initialized` is the structurally correct invariant.

**Why the default-scene (CPU-upload) path is unaffected by Bug W3-T1:**

- Default scene → `want_gpu_producer = false` → seed gate `(!want_gpu_producer || ...)` = `(true || ...)` = `true` → seed runs the very first prepare-frame all resources exist, BEFORE the first regime-2 round. Order is correct by accident. The W5 path's `want_gpu_producer = true` is what flips the gate's polarity and inverts the seed-vs-compute order.

## Confidence level

**HIGH.** The bug is structural and provable from source:

- The gate at `mod.rs:1646` makes the seed dispatch contingent on `gpu_producer_has_run`, which is only flipped in the Core3d schedule by `naadf_gpu_producer_node` (`mod.rs:2635, 2704`).
- The chain at `mod.rs:296-307` runs `naadf_gpu_producer_node` immediately followed by `naadf_bounds_compute_node` in the same frame.
- `naadf_bounds_compute_node` (`bounds_calc.rs:311-370`) has no `bounds_initialized` check.
- The CPU-seeded `bound_queue_info[0..2].size = 32768` + zero-initialized `bound_group_queues` is an internally inconsistent state that the regime-1 seed shader is responsible for fixing.
- The default-scene path (which works correctly) takes the opposite branch of the seed gate by virtue of `want_gpu_producer = false`, which mechanically reverses the timing and produces correct ordering.

The mechanism explains both observed symptoms (inverted distant surfaces + short render distance) via the same root cause: chunk-layer AADFs converge only for group `(0,0,0)`; everywhere else, the renderer's `shoot_ray` reads `boundsInDir = 0` for empty chunks and single-steps along the chunk grid, exhausting its march budget before reaching distant voxels and producing the visual artifacts the user reports.

## Cross-references

- Production code:
  - `crates/bevy_naadf/src/render/construction/mod.rs:1315-1391` — bound-queue family allocation + CPU seed.
  - `crates/bevy_naadf/src/render/construction/mod.rs:1643-1672` — regime-1 seed dispatch (timing-broken gate).
  - `crates/bevy_naadf/src/render/construction/bounds_calc.rs:311-370` — `naadf_bounds_compute_node` (missing `bounds_initialized` gate).
  - `crates/bevy_naadf/src/render/mod.rs:296-307` — Core3d node order.
  - `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` — WGSL port (correct).
- C# reference:
  - `/mnt/archive4/DEV/NAADF/NAADF/Content/shaders/world/data/boundsCalc.fx` — HLSL source (correct).
  - `/mnt/archive4/DEV/NAADF/NAADF/World/Data/WorldBoundHandler.cs:53-89, 91-121` — `Initialize()` runs synchronously before any `Update()`.
- Prior diagnostic:
  - `docs/orchestrate/vox-gpu-rewrite/12-diagnostic-byte-diff-concrete.md` — proves W5 producer chain is byte-correct; localizes remaining bug to W3 / buffer-glue / renderer-addressing.
  - `docs/orchestrate/vox-gpu-rewrite/10-diagnostic-encoding-comparison.md` § "Bug D1" — companion CPU-mirror unpopulated bug (symptom 2) already fixed; the W3 bug here addresses symptoms 1 + 3.
  - `docs/orchestrate/vox-gpu-rewrite/03-impl.md:1328-1450` — Stage 1.5's fix to `bound_group_queue_max_size = 32768` (correct; this diagnostic verified the field is properly written by the W5 per-segment loop at `mod.rs:2524`).
- Original W3 design:
  - `docs/orchestrate/naadf-bevy-port/15-design-c.md` §1.2 regime-1 / regime-2 split.
  - `docs/orchestrate/naadf-bevy-port/16-impl-c-W3.md:141-150` decision #5 — "seed lives in prepare_construction" rationale; the W3 implementer chose this location partly to consolidate W3 buffer allocation, but did not anticipate the W5 path's gate inversion (W5 had not yet landed when W3 was implemented).
