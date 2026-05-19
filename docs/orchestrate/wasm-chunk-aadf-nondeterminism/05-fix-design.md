# Fix design — wasm chunk-AADF cross-pass atomic invisibility

## Problem statement (verbatim from brief)

> On wasm32 / WebGPU via Chrome+Dawn, the regime-2 background bounds-compute
> loop's cross-pass atomic visibility on the `bound_queue_info` storage
> buffer is broken. `prepare_group_bounds`'s
> `atomicLoad(&bound_queue_info[qi].size)` does NOT see writes performed by
> the immediately-preceding `compute_group_bounds` pass's
> `atomicAdd(&bound_queue_info[qi'].size, 1u)` (qi' ≠ qi). The visibility
> failure is TOTAL on web (re-enqueued queues appear permanently empty) and
> absent on native Vulkan (writes propagate normally). Per the probe-1B data:
> native progresses through every (size, axis) queue once and converges in 93
> prepare calls; web drains only the size-0 queues linearly 4096-by-4096 and
> never sees size-1+ queues populated, never reaches convergence within the
> gate window. The observable user-facing symptom is run-to-run SSIM variance
> 0.69-0.94 on the `e2e/tests/vox-horizon-parity.spec.ts` gate, because how
> far the web converges by gate-end is wall-clock-bound.

## Key facts the design must respect

1. **H1 is empirically confirmed.** From `04-probe1-impl.md` cross-target
   table, web's first 200 `prepare_group_bounds` calls are
   byte-for-byte deterministic across 3 runs — they drain queue
   `size0_ax0` linearly (32768→28672→…→4096→0) before moving to
   `size0_ax1`, then `size0_ax2`. Native walks the full size×axis ladder.
   The web `atomicLoad` never sees any size-≥1 queue populated despite the
   compute pass clearly calling `atomicAdd(&bound_queue_info[next_size,
   axis].size, 1u)` (`bounds_calc.wgsl:426`) on every workgroup that grew
   its AADF in the prior pass.

2. **The bug is cross-pass cross-slot.** `atomicStore` to the SAME slot
   `bound_queue_info[size0_ax0].size` from prepare in pass N IS visible to
   the next prepare in pass N+2 (the linear 32768→28672 drain proves it).
   What's invisible is `atomicAdd` to slot `[size1_ax0]` (or any
   `qi' ≠ qi`) from compute in pass N+1 to prepare's `atomicLoad` in pass
   N+2.

3. **Current branch state matters.** The handoff references constants and
   code paths from a *prior session's attempts* that are NOT in this
   worktree:
   - `WASM_MAX_GROUP_BOUND_DISPATCH = 4096` does not exist in the source;
     `config.rs:227` and `config.rs:163` both pin
     `max_group_bound_dispatch: 512 * 64 = 32768` for every target.
   - `bounds_calc.rs:365-417` "wasm-only direct-dispatch branch" does not
     exist; the file is 417 lines total and `dispatch_regime_2_rounds`
     (`bounds_calc.rs:252-289`) uses one encoder + `dispatch_workgroups_indirect`
     on every target.
   - The per-round encoder+submit wasm-only branch the handoff describes
     is also not in this worktree.

   The forbidden moves remain forbidden (raising the 4096 cap, lowering
   the SSIM floor, raising `MAX_RAY_STEPS_PRIMARY`), but the design
   doesn't have a "4096 dispatch ceiling" baseline to work against.

4. **The faithful-port constraint.** C# NAADF (`WorldBoundHandler.cs`)
   runs the prepare+compute loop on D3D11 where cross-pass UAV
   `RWStructuredBuffer<uint>::InterlockedAdd` writes ARE visible to a
   subsequent pass's `InterlockedRead` on the same UAV (D3D11 inserts the
   compute-shader-UAV barrier between dispatches). The Bevy native
   (Vulkan) port has the same guarantee. The Dawn/WebGPU lowering loses
   the `Coherent`/`MakeAvailable+MakeVisible` decoration on the SPIR-V
   variable, which is what the diagnosis names as the root cause.

5. **The existing GPU→CPU readback primitive** at
   `mod.rs:1120-1155` is the template for any new readback chain:
   `copy_buffer_to_buffer` into a `MAP_READ|COPY_DST` staging buffer,
   `map_async` with an `Arc<AtomicBool>` callback, drain
   `device.poll(PollType::Poll)` once per frame, advance a stage enum
   when the bool flips. This is target-agnostic (the existing system
   works on wasm32, per the W3 prepare seed it gates on).

6. **There is no cross-pass `deviceBarrier()` in WGSL.** Per
   `01-diagnostics-design.md` §B.2: WGSL only exposes `storageBarrier` /
   `workgroupBarrier`, both workgroup-scope. There is no
   user-controllable WGSL primitive that adds the missing SPIR-V
   `Coherent` decoration on Dawn — fixing it inside WGSL alone is
   structurally impossible.

## Design space exploration

### Shape A — Submit-boundary fence per round on wasm (revisit the prior session's "neutral" attempt)

- **Shape:** wasm-only cfg-gate at the dispatch site — replace the single
  encoder loop with `n_rounds` encoders, one per {prepare, compute}
  round, each submitted independently via `render_queue.submit(...)`.
  Forces a `vkQueueSubmit` boundary between rounds; Dawn must serialise
  the queue timeline so the cross-pass atomic write becomes visible to
  the next encoder's first read.
- **Mechanism:** Vulkan queue-submit injects an implicit memory
  dependency on the global queue timeline. Per `01-diagnostics-design.md`
  §B.1, "Dawn's tracker is per-encoder; across `commandEncoder.finish() +
  device.queue.submit([cb1, cb2])` Dawn relies on the underlying Vulkan
  driver's queue-timeline semantics to provide ordering." So even if the
  intra-encoder `vkCmdPipelineBarrier` is missing the `Coherent` decoration
  Dawn-side, the queue-timeline submit fence force-flushes any prior
  writes between submits.
- **Performance:** N extra `commandEncoder.finish()` + `submit()` calls
  per node invocation (5 per frame at `n_bounds_rounds = 5`). Per-submit
  cost on Chrome/Dawn is roughly 100µs–500µs of JS bridge overhead;
  5 extra submits/frame = ~2.5ms at the worst end of that range. At 60fps
  the budget is 16.7ms, so this is a measurable but not catastrophic
  fraction.
- **Native impact:** None if cfg-gated; the native one-encoder path is
  preserved.
- **Faithful-port impact:** Divergent from C#'s "one buffered dispatch
  list" (D3D11 internally batches), but only wasm-side. The semantic
  equivalence holds: each round still runs prepare→compute exactly once.
  No algorithmic divergence.
- **Risk:** The handoff says this was already tried and the SSIM result
  was *neutral* at ~0.79. The diagnosis (`03-diagnosis.md` Section F
  Hypothesis 3) interprets that neutral result as evidence the
  cross-encoder *queue-timeline* fence is also insufficient — the bug is
  in Dawn's cross-pipeline tracking even WITH separate submits. **The
  probe-1B data does NOT directly test this hypothesis** because the
  per-round-submit code is not in the current worktree. Falsification
  line: if applied alone, SSIM stays in the 0.78–0.81 deterministic
  cluster.

### Shape B — Single `atomic<u32>` flat slot table; eliminate the cross-slot atomic-store/atomic-load pattern

- **Shape:** Refactor `bound_queue_info` from `array<BoundQueueInfo>`
  (struct with `start: u32` + `size: atomic<u32>`) into TWO separate
  flat storage buffers: `bound_queue_starts: array<u32>` (non-atomic;
  only `prepare` writes it, single-thread) and `bound_queue_sizes:
  array<atomic<u32>>` (atomic everywhere). This is the "convert the
  whole field to atomic everywhere" attempt but applied to `size` only
  (the field that genuinely has multi-writer dynamics) and EXPLICITLY
  declaring the storage as a top-level `array<atomic<u32>>` rather than
  a struct field. The prior attempt to atomicise `bound_refined_info`
  was misguided; this targets the actually-multi-writer field.
- **Mechanism:** Top-level `array<atomic<u32>>` declarations in WGSL
  produce a different Tint lowering than struct fields that contain
  `atomic<u32>`. Specifically, `bound_group_masks: array<atomic<u32>>`
  already works correctly on web (per the probe-1A data, masks are
  consumed correctly within the same compute pass). Moving `size` out
  of the struct + declaring it as `array<atomic<u32>>` matches the
  working pattern.
- **Performance:** Negligible — same buffer count, same total memory
  (96 × 4B each instead of 96 × 8B combined). Two binding slots in
  `@group(1)` instead of one combined slot.
- **Native impact:** Touches a layout shared with native (every pipeline
  rebinds `@group(1)` with a new entry). Not cfg-gated. Native must
  continue to pass the convergence unit-test
  (`bounds_calc_convergence_matches_cpu_oracle`,
  `tests.rs:604-708`).
- **Faithful-port impact:** Changes the GPU type relative to C#'s
  `RWStructuredBuffer<BoundQueueInfo>` (a packed start/size struct).
  Diverges from the canonical NAADF GPU layout. Documentable as a
  Bevy-only buffer split (the WGSL → SPIR-V lowering is
  implementation-detail of the WebGPU port; the algorithm is
  unchanged).
- **Risk:** The prior session attempted exactly the symmetric move on
  `bound_refined_info` and it regressed. The hypothesis is that THIS
  field (`bound_queue_info.size`) is the right one to atomicise as
  flat — but this is hypothesis-driven, not empirically tested.
  Falsification: web SSIM stays at 0.78-0.81 cluster despite the
  refactor.

### Shape C — CPU mirror of `bound_queue_info.size`; round the loop through host memory

- **Shape:** On wasm only, between rounds, GPU→CPU read back the 96-u32
  `bound_queue_info.size` field, then CPU→GPU write_buffer it back to
  the same buffer with the host-known values. The CPU readback +
  re-upload acts as a forced memory-coherence barrier — any pending
  cross-pass writes MUST be visible on CPU readback (per the WebGPU
  spec's `mapAsync` guarantee), and the write_buffer pushes the
  authoritative values forward into the next round.
- **Mechanism:** Per `01-diagnostics-design.md` §B.6, `mapAsync()`
  "guarantees that all submitted work whose execution time-ordering
  precedes this call has completed." This is a STRONGER guarantee than
  cross-pass barriers — it serialises through the host. A host-side
  `queue.write_buffer` after the readback then re-asserts the value
  into the next GPU round's view.
- **Performance:** A `map_async` per round at 60fps × 5 rounds/frame =
  300 map_asyncs/sec. Each readback is 96 × 4B = 384 B. The latency to
  a `map_async` callback resolving on Chrome/Dawn is typically several
  frames (the existing `populate_cpu_mirror_from_gpu_producer`
  documentation at `mod.rs:1042-1465` shows it takes multiple frames
  for the first `cursor_done` callback to fire on web). At 60fps that's
  a per-round latency of ~50ms minimum — **catastrophically slow** for
  a per-round fence.
- **Native impact:** Cfg-gated; native runs the existing path unchanged.
- **Faithful-port impact:** Hard divergence from C# (C# never reads back
  bound queue state). Algorithmic equivalent (the values written back
  are the values that would have been there), but architecturally
  alien.
- **Risk:** The per-round latency dominates frame budget. The whole
  point of the regime-2 loop is to do *many* rounds per frame; if each
  round costs 50ms+, the convergence rate drops by 50× compared to
  native's 5 rounds/frame. **Net: the algorithm finishes in 50 frames
  on native, 2500+ frames on wasm** — strictly worse than the current
  bug. Falsification: this is structurally a non-starter, falsified by
  the existing readback latency observation.

### Shape D — Move the per-queue `start` cursor out of `bound_queue_info` so prepare can read+write without crossing the atomic slot's cache line

- **Shape:** Keep `bound_queue_info.size: atomic<u32>` but split out
  `start` into a separate non-atomic `bound_queue_starts: array<u32>`
  buffer. The hypothesis is that Tint's WGSL→SPIR-V lowering treats a
  struct containing `atomic<u32>` as a single "atomic-decorated"
  variable, where reads of the `start` field accidentally share the
  cache-coherence treatment of the `size` field but with the wrong
  decoration. Separating them might unstick the cross-pass visibility.
- **Mechanism:** Speculative — based on the diagnosis's note that Tint
  may misclassify the buffer's read pattern based on the struct's mixed
  atomic+non-atomic content.
- **Performance:** Marginal — two buffers instead of one, same total
  size, same number of memory loads.
- **Native impact:** Layout change touches native path (non-cfg-gated).
  Same as Shape B's risk surface.
- **Faithful-port impact:** Same as Shape B — diverges from C#'s
  packed struct.
- **Risk:** This is purely speculative; no probe data supports the
  "mixed-atomicity struct misclassifies the lowering" hypothesis. The
  diagnosis explicitly says (Hypothesis 1) "if Tint emits the
  storage-buffer access without the `Coherent` / `MakeAvailable`
  decoration on the `bound_queue_info` buffer". The decoration is
  buffer-level; splitting the struct wouldn't add the decoration.
  Falsification: same SSIM cluster.

### Shape E — Host-driven prepare; do `prepare_group_bounds`'s queue picking on CPU, drive `compute_group_bounds` from the host via `queue.write_buffer` and indirect-dispatch args

- **Shape:** Move the `prepare_group_bounds` queue-selection logic
  entirely to the host. CPU reads (via single readback at frame start)
  the current `bound_queue_info` contents, picks the next non-empty
  queue, writes the `bound_refined_info` and `bound_dispatch_indirect`
  buffers via `queue.write_buffer`, then dispatches `compute_group_bounds`.
  Eliminates the GPU-side prepare pass entirely.
- **Mechanism:** Bypasses the cross-pass atomic visibility issue by
  routing the queue-selection signal through host memory, which has
  guaranteed coherence semantics.
- **Performance:** Each round requires a CPU readback of
  `bound_queue_info`. Same latency issue as Shape C (50ms+ per
  readback on Chrome). Plus the CPU now contains a port of
  `prepare_group_bounds`'s 96-iteration `for` loop — but that's trivial
  CPU work.
- **Native impact:** Cfg-gated; native keeps the GPU prepare path.
- **Faithful-port impact:** Major — moves an algorithmic step from GPU
  to CPU. The C# WorldBoundHandler does NOT do this. Architecturally a
  significant divergence.
- **Risk:** Same latency wall as Shape C. The fundamental issue is
  that GPU→CPU readback on Chrome is too slow to be a per-round
  primitive. **Net: same falsification as Shape C.**

### Shape F — `copy_buffer_to_buffer` self-copy between rounds (the "Dawn cache-line eviction" trick)

- **Shape:** Between rounds, on wasm only, insert an encoder operation
  that copies `bound_queue_info` to itself (or to a scratch buffer and
  back). On Dawn, `copy_buffer_to_buffer` issues a Vulkan
  `vkCmdCopyBuffer` which carries an implicit
  `VK_PIPELINE_STAGE_TRANSFER_BIT` + `VK_ACCESS_TRANSFER_WRITE_BIT`
  barrier. The hope is that the transfer-stage barrier forces a flush
  of the prior compute-stage writes, making them visible to the next
  compute-stage's reads.
- **Mechanism:** The Vulkan spec requires a `TRANSFER_WRITE → SHADER_READ`
  barrier across pipeline stages. Dawn must insert that barrier when it
  sees a buffer used as `STORAGE_WRITE` then `TRANSFER_WRITE` then
  `STORAGE_READ`. If Dawn's tracker handles the TRANSFER intermediate
  correctly even when the cross-storage-storage tracking is broken,
  this acts as a forced barrier.
- **Performance:** A `copy_buffer_to_buffer` of 768 B (the size of
  `bound_queue_info`) per round. Roughly the same cost as a buffer
  write — cheap.
- **Native impact:** Cfg-gated.
- **Faithful-port impact:** Cfg-gated wasm-only barrier injection, no
  algorithmic change.
- **Risk:** Speculative. There is no documented evidence Dawn's
  cross-stage barriers are tracked correctly when the cross-encoder
  shader-shader path is broken. **However:** the "Dawn does cross-stage
  barriers correctly even when atomic-storage tracking is buggy" is a
  *typical* GPU-driver bug pattern (intra-stage tracking is the rarest
  surface; cross-stage barriers are exercised by every renderer).
  Falsification: SSIM stays in the broken cluster.

### Shape G — Replace the atomic re-enqueue with a queue-rebuild step (scan-based)

- **Shape:** Refactor the algorithm. After every N rounds of
  prepare+compute, run a "queue rebuild" kernel that scans
  `bound_group_masks` and rebuilds `bound_queue_info`/`bound_group_queues`
  from scratch. The masks ARE consumed correctly (probe-1B doesn't show
  mask-side errors; the mask is `array<atomic<u32>>` flat, the same
  pattern Shape B proposes for `size`). Rebuilding from the masks
  bypasses the cross-pass atomic-add-then-atomic-load chain entirely.
- **Mechanism:** The masks are the *authoritative* state; the queues
  are an index for fast lookup. If the masks survive a round correctly
  (single-pass `atomicOr`/`atomicAnd`, no cross-pass cross-slot
  dependency), then a queue rebuild reads the mask and emits the queue
  contents from scratch.
- **Performance:** Adds a full-grid scan kernel periodically. At
  `bound_group_count = 32768` (Oasis), one kernel pass over 32K groups
  at one workgroup per 64 groups = 512 workgroups, trivial.
- **Native impact:** Algorithmic change touching every target. Not
  cfg-gated (the queue rebuild step happens on all targets). Native
  must continue to converge correctly with the rebuild added.
- **Faithful-port impact:** Major. C# does not do this; the C#
  algorithm relies on the queue being incrementally maintained by
  prepare+compute. Significant divergence.
- **Risk:** Algorithmic surface is large. Must be carefully placed in
  the round loop so it doesn't interact pathologically with the
  in-flight queue state. The unit-test surface
  (`bounds_calc_convergence_matches_cpu_oracle`) checks final
  convergence, which the rebuild approach should still satisfy, but
  testing the intermediate state across the rebuild boundary is new
  surface. Falsification: convergence test fails, or the rebuild
  kernel mis-computes the queue contents.

### Shape H — Combination: per-round submit fence (A) + Dawn-targeted barrier hint (transfer-stage no-op copy, F)

- **Shape:** On wasm32 only: per-round encoder+submit, AND after the
  compute pass within each round, insert a no-op
  `copy_buffer_to_buffer(bound_queue_info, scratch, …)` followed by
  `copy_buffer_to_buffer(scratch, bound_queue_info, …)`. Two
  intentional barrier-inducing transfers across the cross-pass
  boundary.
- **Mechanism:** A∪F. The handoff documents that A alone moves SSIM
  from "broken" (~0.30 implied) to "marginal" (~0.79). The
  cross-stage transfer (F) closes the cross-pass-cross-slot atomic
  visibility gap that the queue-timeline fence alone might not.
- **Performance:** Sum of A and F: 5 extra submits/frame + 5 extra
  copy_buffer_to_buffer roundtrips of 768B. Both small. Total budget
  impact ~3ms/frame at worst.
- **Native impact:** Cfg-gated.
- **Faithful-port impact:** Cfg-gated; no algorithmic change.
- **Risk:** If A alone is insufficient (per diagnosis Hypothesis 3),
  and F doesn't add a usable barrier on Dawn, this still fails.
  Falsification: SSIM stays at 0.79 cluster.

## Recommended approach

### Recommendation: Shape B (atomic-flat refactor of `bound_queue_info.size`), as the primary fix.

If Shape B alone proves insufficient (web SSIM stays in the 0.78–0.81
deterministic cluster, indicating the cross-pass visibility issue
persists), the step-2 fallback is to layer Shape A (wasm-only per-round
encoder+submit) on top. The combined Shape B+A is the architecturally
clean equivalent of the prior session's "atomicise + submit fence" idea
applied to the *correct* field.

### Why Shape B wins

1. **The probe-1B data identifies the SPECIFIC field that is
   cross-pass-cross-slot invisible: `bound_queue_info[qi'].size` written
   by `atomicAdd` at `bounds_calc.wgsl:426`.** No other field exhibits
   this pattern: `bound_group_masks` already works (it's
   `array<atomic<u32>>` flat); `bound_refined_info` is single-writer
   per-call; `bound_dispatch_indirect` is single-writer per-call;
   `bound_group_queues` is single-writer-per-slot. The narrow refactor
   targets the *exact* surface where the bug manifests.

2. **The pattern `array<atomic<u32>>` is empirically known to work
   correctly on Dawn for the same algorithm in the same shader.**
   `bound_group_masks` is declared at `bounds_calc.wgsl:107` as
   `array<atomic<u32>>` and Tint lowers it with the correct
   coherence decorations — the masks ARE consumed correctly cross-pass
   on web (probe-1B's deterministic cross-run behaviour rules out
   mask-side races as a separate bug surface). Moving `size` to the
   same shape adopts a proven-working Dawn lowering.

3. **It does not require submit-fence overhead.** Shape A imposes a
   per-frame budget of ~2.5ms/frame in wasm submit overhead. Shape B
   is a layout refactor with zero runtime cost.

4. **It cfg-isolates poorly — but the diagnosis confirms the bug is
   purely a Dawn-lowering issue, not an algorithmic one, so the change
   has to be at the WGSL source level.** This is the one place where
   the design DOES touch native code paths. The unit test
   `bounds_calc_convergence_matches_cpu_oracle` at `tests.rs:604-708`
   is the existing safety net: native must continue to converge to
   the CPU oracle. The refactor preserves all algorithmic semantics
   (same atomic ops, same logical buffer, same read/write
   discipline).

5. **The faithful-port divergence is minimal and documentable.** C#
   uses `RWStructuredBuffer<BoundQueueInfo>` (a 2-field struct). The
   Bevy port already diverged from C#'s storage layout in other places
   (the chunks-texture-to-buffer migration for web compatibility,
   `bounds_calc.wgsl:96` — `array<vec2<u32>>` instead of the C#
   `RWTexture3D<uint2>`). Splitting `bound_queue_info` is in the
   same class: a Bevy-side buffer layout adjustment for WebGPU
   compatibility, with WGSL semantics preserved.

### Why the alternatives lose

- **A alone:** The diagnosis (Hypothesis 3) interprets prior data as
  evidence A is insufficient. If true, A alone falsifies. Worth
  trying as a step-2 *addition* if B alone is insufficient — but as
  the primary lever, B is more targeted.

- **C, E:** Both require `map_async` per-round. The existing
  readback's documented latency (`mod.rs:1196` "stage NotStarted →
  CursorPending" then multi-frame wait) is too slow to be a per-round
  primitive. Falsified structurally.

- **D:** Pure speculation; no probe data supports the
  mixed-atomicity-struct-misclassification hypothesis. Lower
  confidence than B.

- **F:** The "cross-stage barrier as a workaround" is plausible but
  has no documented evidence in the diagnostics. B is more direct.

- **G:** Algorithmic refactor; large surface; high faithful-port
  divergence; would require user approval per the project's
  faithful-port rule.

- **H:** A+F combined; same speculation as F.

### Why a layered Shape B + Shape A fallback is the right risk posture

The probe-1B data confirms what's broken (cross-pass cross-slot atomic
visibility); it does not prove WHICH lowering decoration Tint is
missing. Shape B addresses the most likely cause (the buffer-level
atomic-decoration shape). If wrong, the per-round submit fence is the
strictly-stronger fallback (queue-timeline visibility is mandated by
WebGPU spec). The two together are belt-and-suspenders.

The impl should land Shape B *first*, verify the gate (3 SSIM runs),
and ONLY add Shape A if B alone is insufficient. This keeps the diff
minimal and isolates the cause.

## Step-by-step implementation plan

### Step 1 — WGSL refactor: split `bound_queue_info`'s `size` into a flat `array<atomic<u32>>`

**File:** `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl`

- **Edit at `bounds_calc.wgsl:56-60`:** Delete the `BoundQueueInfo`
  struct definition entirely.
- **Edit at `bounds_calc.wgsl:101-109`:** Replace the single
  `@group(1) @binding(0) var<storage, read_write> bound_queue_info:
  array<BoundQueueInfo>;` with TWO bindings:

  ```wgsl
  @group(1) @binding(0)
  var<storage, read_write> bound_queue_starts: array<u32>;       // non-atomic; written only by prepare (single-thread).
  @group(1) @binding(4)
  var<storage, read_write> bound_queue_sizes: array<atomic<u32>>; // atomic; written by prepare (atomicStore) AND compute (atomicAdd).
  ```

  The other 3 bindings (`bound_group_queues`, `bound_group_masks`,
  `bound_refined_info`) keep their existing binding numbers (1, 2, 3).
  Adding `bound_queue_sizes` at `binding(4)` widens `@group(1)` from 4
  to 5 bindings. This stays well under the `max_storage_buffers_per_shader_stage`
  cap (wasm reports 8 per the snapshot data; native reports higher).

  Note that the original `bound_queue_info` was at binding 0; splitting
  preserves the binding-0 slot for `starts` to minimise the diff
  blast radius on bind-group construction. (See Decisions for the
  rejected "renumber from 0 with both new bindings adjacent" choice.)

- **Edit at every site that accesses `bound_queue_info[qi].size`** —
  the WGSL grep is exhaustive:
  - `bounds_calc.wgsl:278` (`prepare_group_bounds`):
    `let size = atomicLoad(&bound_queue_info[qi].size);`
    → `let size = atomicLoad(&bound_queue_sizes[qi]);`
  - `bounds_calc.wgsl:304` (`prepare_group_bounds`):
    `atomicStore(&bound_queue_info[qi].size, found_size - group_amount);`
    → `atomicStore(&bound_queue_sizes[qi], found_size - group_amount);`
  - `bounds_calc.wgsl:426` (`compute_group_bounds`):
    `let original_size = atomicAdd(&bound_queue_info[qi].size, 1u);`
    → `let original_size = atomicAdd(&bound_queue_sizes[qi], 1u);`

- **Edit at every site that accesses `bound_queue_info[qi].start`:**
  - `bounds_calc.wgsl:277` (`prepare_group_bounds`):
    `let start = bound_queue_info[qi].start;`
    → `let start = bound_queue_starts[qi];`
  - `bounds_calc.wgsl:300-301` (`prepare_group_bounds`):
    `bound_queue_info[qi].start = (found_start + group_amount) % params.bound_group_queue_max_size;`
    → `bound_queue_starts[qi] = (found_start + group_amount) % params.bound_group_queue_max_size;`
  - `bounds_calc.wgsl:429` (`compute_group_bounds`):
    `let queue_start_index = bound_queue_info[qi].start;`
    → `let queue_start_index = bound_queue_starts[qi];`

- **Verification of step 1:** the WGSL file must compile (naga
  validation on native; Tint on wasm). The shader compile happens at
  `cargo build` time on native through the asset pipeline; on wasm
  through the asset bundling. Step 2's bind-group layout must match,
  or pipeline creation fails with a clear error message.

### Step 2 — Rust layout descriptor: widen `construction_bounds_layout`

**File:** `crates/bevy_naadf/src/render/construction/bounds_calc.rs`

- **Edit at `bounds_calc.rs:93-113`:** Update
  `construction_bounds_layout_descriptor()` from 4 bindings to 5:
  insert a 5th `storage_buffer_sized(false, None)` entry corresponding
  to `bound_queue_sizes`. Update the documentation comment to mention
  the split.

  The existing layout at `bounds_calc.rs:93-113` uses
  `BindGroupLayoutEntries::sequential` over a tuple of 4 entries; add a
  5th. The shader's `@group(1) @binding(4)` decoration must align with
  the 5th tuple position (zero-indexed slot 4).

### Step 3 — Rust GPU type: split `GpuBoundQueueInfo`

**File:** `crates/bevy_naadf/src/render/gpu_types.rs`

- **Edit at `gpu_types.rs:660-681`:** Replace `GpuBoundQueueInfo` (the
  struct with `start: u32` + `size: u32`) with two semantic constants
  reflecting the WGSL layout: the two arrays are independent. The
  existing 2-u32-per-entry size assertion at line 679 stays valid (it
  just describes the conceptual pair). Either:
  - **(a)** Keep `GpuBoundQueueInfo` as a documentation-only marker
    and stop uploading via that struct; OR
  - **(b)** Delete `GpuBoundQueueInfo` and document the split in
    comments. Both downstream consumers (`mod.rs:1785-1795` upload
    seed, `tests.rs:455-465` test seed) will need to change to upload
    two separate buffers instead of one struct array.

  Prefer **(b)**: cleaner, fewer dangling abstractions. The 8-byte-pair
  type is just confusing once the actual GPU layout is two flat
  buffers.

### Step 4 — Rust seed allocation: two buffers, not one

**File:** `crates/bevy_naadf/src/render/construction/mod.rs`

- **Edit `ConstructionGpu` struct (around `mod.rs:140` per the grep
  earlier; the actual field is named `bound_queue_info`):** Replace
  the single `bound_queue_info: Option<Buffer>` field with two:
  `bound_queue_starts: Option<Buffer>` + `bound_queue_sizes:
  Option<Buffer>`.

- **Edit `prepare_construction` at `mod.rs:1772-1848`:** Replace the
  single `info_buf` allocation with two:

  ```rust
  let starts_buf = render_device.create_buffer(&BufferDescriptor {
      label: Some("naadf_bound_queue_starts"),
      size: 32 * 3 * 4,  // 96 u32s, 384 B.
      usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
      mapped_at_creation: false,
  });
  let sizes_buf = render_device.create_buffer(&BufferDescriptor {
      label: Some("naadf_bound_queue_sizes"),
      size: 32 * 3 * 4,  // 96 u32s, 384 B.
      usage: BufferUsages::STORAGE | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
      mapped_at_creation: false,
  });
  // Seed: starts all zero; sizes[i*3+xyz] = (i == 0 ? bound_group_count : 0).
  let mut sizes_seed: Vec<u32> = Vec::with_capacity(32 * 3);
  for i in 0..32u32 {
      for _xyz in 0..3u32 {
          sizes_seed.push(if i == 0 { bound_group_count } else { 0 });
      }
  }
  render_queue.write_buffer(&starts_buf, 0, bytemuck::cast_slice(&[0u32; 96]));
  render_queue.write_buffer(&sizes_buf, 0, bytemuck::cast_slice(&sizes_seed));
  ```

  Assign both into `gpu.bound_queue_starts` and `gpu.bound_queue_sizes`.

- **Edit the bind-group construction site** (search for the
  `construction_bounds` bind group build; it currently passes 4
  bindings to `BindGroupEntries::sequential`): widen to 5 bindings,
  with slot 0 = starts, slots 1-3 = queues/masks/refined (unchanged),
  slot 4 = sizes.

### Step 5 — Rust unit-test fixture update

**File:** `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs`

- **Edit `W3Fixture` at `tests.rs:411-428`:** Replace
  `bound_queue_info: Buffer` with two fields: `bound_queue_starts:
  Buffer` and `bound_queue_sizes: Buffer`.

- **Edit `build_w3_fixture` at `tests.rs:455-465`:** Replace the
  `info_seed` Vec construction with two seeds (a `starts_seed: Vec<u32>`
  of 96 zeros and a `sizes_seed: Vec<u32>` from the same logic as
  step 4). Create two buffers; assign both into the fixture.

- **Edit the bounds bind-group construction at `tests.rs:547-555`:**
  Widen from 4 entries to 5, slot 0 = starts, slot 4 = sizes.

- **Edit the field-keep-alive references at `tests.rs:698-707`:**
  Update the `let _ = (...)` tuple to reference `bound_queue_starts`
  and `bound_queue_sizes` instead of `bound_queue_info`.

### Step 6 — Probe-1B teardown decision

**File:** `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` (probe
binding), `crates/bevy_naadf/src/render/construction/mod.rs` (probe
host wiring), `crates/bevy_naadf/src/render/construction/bounds_calc.rs`
(probe layout descriptor + pipeline wiring), `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs`
(test fixture probe wiring).

**Decision:** Keep the probe-1B instrumentation IN-PLACE through the
gate runs. Rationale:
- The probe is purely observational and adds <1µs per prepare call
  (one shader store).
- Keeping it lets the impl agent observe whether the fix actually
  changes the prepare-call sequence (post-fix web should look like
  native: ~93 calls across all (size, axis) pairs, not 200+ calls
  draining size-0 only).
- Removing it before verification would lose the diagnostic surface
  that just confirmed H1.
- The probe binding is `@group(3) @binding(0)` and only on the
  prepare pipeline; doesn't interact with the bind-group changes for
  `@group(1)` in steps 1-5.

After the gate passes 3× consecutively at SSIM ≥ 0.91, a follow-up
patch (post-fix) can remove the probe to drop the diagnostic
overhead. Out of scope for THIS fix.

### Step 7 — Verification of step 1-5: `cargo test --workspace --lib`

Runs the existing `bounds_calc_convergence_matches_cpu_oracle` unit
test (`tests.rs:604`) on the native test runner. This is the
unit-level safety net for the WGSL refactor. The test:
- Builds a 4×4×4 chunk world.
- Runs regime-1 seed + 200 rounds of regime-2.
- Reads back the chunks buffer.
- Asserts every empty chunk matches the CPU oracle's converged value.

If the WGSL refactor introduced a bug (wrong binding number, wrong
buffer offset, lost atomicStore), this test will fail on native
**deterministically**. It is the right pre-flight gate before the
slower e2e gates.

**Verification command:**
```
cargo test --workspace --lib bounds_calc_convergence_matches_cpu_oracle -- --exact
```

If this fails, fix and re-run before progressing.

### Step 8 — Verification of step 1-5: `cargo run --bin e2e_render -- --vox-horizon-native`

Re-capture the native reference image. Native must still produce
the same `vox_horizon_native.png` (or at minimum, an image that
SSIMs ≥ 0.99 to the previous reference; we are not changing
algorithm, only buffer layout, so byte-equality is the expectation).

**Verification command:**
```
cargo run --release --bin e2e_render -- --vox-horizon-native
```

Then optionally `pixelmatch` / SSIM between the prior committed
`vox_horizon_native.png` and the new one (the e2e_render gate's own
similarity check, if there is one for native-native runs).

### Step 9 — Verification: web build + parity gate

```
just web-build-release
cd e2e && timeout 240s npx playwright test vox-horizon-parity.spec.ts --headed
```

This is the bug's user-facing surface. The SSIM must be ≥ 0.91.

### Step 10 — Stability verification — 3 consecutive parity-gate runs

Run the parity gate **3 separate times** (each a fresh
`web-build-release` is NOT needed; the same wasm bundle suffices for
3 consecutive `playwright test` invocations). Record each SSIM. The
PASS condition: **ALL THREE runs must have SSIM ≥ 0.91**. Not the
median, not the average — every one of three.

**Verification command per run:**
```
cd e2e && timeout 240s npx playwright test vox-horizon-parity.spec.ts --headed
```

Capture the SSIM from each run's Playwright log. Persist all three
SSIM values to the impl log along with the post-fix probe-1B output
(prepare-call sequence).

### Step 11 — If step 10 fails (any of 3 < 0.91): add Shape A on top

**Only execute step 11 if step 10 has at least one SSIM < 0.91 across
3 runs.**

Re-shape `dispatch_regime_2_rounds` at `bounds_calc.rs:252-289` to
cfg-gate the wasm32 path: instead of one encoder for `n_rounds`
prepare+compute pairs, do `n_rounds` separate encoders, each
finished and submitted via `render_queue.submit([encoder.finish()])`.

Sketch (wasm-only branch):

```rust
#[cfg(target_arch = "wasm32")]
{
    let device = render_device.as_ref();
    let queue = render_queue.as_ref();
    for _ in 0..n_rounds {
        let mut enc = device.create_command_encoder(&CommandEncoderDescriptor {
            label: Some("naadf_bounds_calc_round_enc_wasm"),
        });
        // ... begin prepare pass, begin compute pass ...
        queue.submit([enc.finish()]);
    }
    return;
}
// native: existing one-encoder body.
```

But: `dispatch_regime_2_rounds` is called with an `encoder` argument
borrowed from the render-graph node. The render-graph encoder is
owned by `RenderContext`. We CANNOT take a separate device + queue
inside this function without changing the signature.

Two sub-shapes for step 11 if it's needed:
- **Step 11a:** Change `naadf_bounds_compute_node` (the caller) at
  `bounds_calc.rs:311-385` to cfg-gate wasm32: instead of calling
  `dispatch_regime_2_rounds(encoder, ..., n_rounds)`, on wasm
  bypass the render-graph encoder and use `render_device.create_command_encoder`
  + `render_queue.submit` per round. This requires plumbing
  `RenderDevice` + `RenderQueue` into the node (currently it only
  takes `RenderContext`). Bevy 0.19 makes this available via
  `Res<RenderDevice>` + `Res<RenderQueue>` in the system signature.
- **Step 11b:** Extract a helper `dispatch_one_regime_2_round`
  (single prepare+compute pair on a given encoder) and call it from
  both the native one-encoder loop and a new wasm-only per-encoder
  loop. Cleaner separation; lower risk.

Pick 11b — keeps the helper testable. Re-run step 10 after applying.

If step 10 still fails after Shape B+A: **stop and re-orchestrate.**
The probe-1B data plus Shape B+A failing rules out both the
cross-pass barrier and the submit-fence as sufficient. The remaining
hypothesis space (G, or a Tint-version regression) requires user
input.

### Step 12 — Optional probe-1B removal

If steps 7-10 all pass, file a follow-up patch (not part of THIS fix
PR per the user's instruction not to bundle scope-creep) to remove
the probe-1B instrumentation: revert the `@group(3)` binding in
WGSL, the `prepare_probe_history` field on `ConstructionGpu`, the
extra layout, the `aadf_per_call_probe` system. Out of scope for
this design.

## Test impact

### Existing tests in `bounds_calc/tests.rs`

| Test | Status under Shape B | Rationale |
|---|---|---|
| `bounds_calc_convergence_matches_cpu_oracle` (tests.rs:604) | **Updates required** (fixture changes per step 5); should still PASS | The test runs on native; convergence semantics unchanged. Fixture must allocate two buffers instead of one. |
| `bounds_queue_no_overrun` (tests.rs:717) | **Updates required** (fixture changes per step 5); should still PASS | Same. |
| `bounds_per_axis_atomic_correctness` (referenced at tests.rs:17) | **Updates required** (fixture changes per step 5); should still PASS | Same. |

All three tests rebuild the fixture and exercise the new bind-group
shape. The W3 fixture refactor in step 5 covers all three tests via
the shared `build_w3_fixture` helper.

### New tests proposed

**None required for this fix.** The unit-test surface already exists
and is the right gate (it verifies convergence against the CPU
oracle). Adding a "the two buffers stay in sync" test is redundant —
that's exactly what the convergence test verifies.

A *speculative future* test surface — a wasm-targeted unit test that
verifies the cross-pass atomic-visibility on Dawn directly — would
be useful but is **out of scope for this fix**. It would require:
- A wasm-specific test runner (the existing tests run on native via
  `cargo test`).
- A minimal shader that mirrors the prepare→compute round.
- A readback to assert the size-1 queue is non-zero after one round.

This is a diagnostic primitive, not a fix primitive. If the next
session finds itself needing one, it can be added then.

### The e2e gate `e2e/tests/vox-horizon-parity.spec.ts`

The SSIM ≥ 0.91 (`HORIZON_SSIM_SIMILARITY_MIN` at
`vox_horizon_parity.rs:133`) gate IS the user-facing verification
surface. Step 10's 3× stability run is the load-bearing check.

## Stability verification plan

The bug is non-deterministic (historical SSIM 0.69–0.94 run-to-run).
The fix must produce SSIM ≥ 0.91 **in ALL of 3 consecutive parity-gate
runs**, not just one and not just the median.

**Procedure:**

1. Apply Shape B (steps 1-6 above).
2. Pass the unit-test gate (step 7).
3. Pass the native-reference gate (step 8).
4. Build the web bundle once (`just web-build-release`).
5. Run the parity gate (step 9). Record SSIM.
6. Run the parity gate again. Record SSIM.
7. Run the parity gate a third time. Record SSIM.
8. ALL 3 SSIMs must be ≥ 0.91.

If any of the 3 runs is < 0.91, layer in Shape A (step 11) and
repeat 4-8 (re-build, 3 runs, all ≥ 0.91).

If ALL 3 are ≥ 0.91 on the first attempt (Shape B alone): the fix is
complete. Do NOT add Shape A — it would be speculative scope-creep.

**Probe-1B confirmation** (additive, not gate-blocking):

The probe-1B instrumentation is left in place through verification.
On a fixed web run, the probe-1B output should look like the native
baseline: ~93 prepare calls across all (size, axis) pairs with
`found_size=32768`, then NONE entries. NOT the 200+ size-0-only
calls the bug produces. This is a secondary signal that the fix
addressed the root cause vs masked the symptom.

## Decisions & rejected alternatives

1. **Decision: Pick Shape B (atomic-flat refactor) as primary, not
   Shape A (per-round submit fence).**
   - Rejected: Shape A as primary. The handoff reports A alone moved
     SSIM from "broken" to "marginal" but not to "passing"; the
     diagnosis interprets this as evidence the cross-encoder fence is
     necessary but insufficient. Shape B is more targeted at the
     specific mechanism the probe-1B data confirms.
   - Falsifier: If post-fix SSIM stays in the 0.78–0.81 deterministic
     cluster, Shape B alone is wrong; add Shape A.

2. **Decision: Add `bound_queue_sizes` at `@group(1) @binding(4)`
   (preserves binding 0 as the renamed `bound_queue_starts`).**
   - Rejected: Renumbering all of `@group(1)` to put the two new
     bindings adjacent at slots 0+1. That would force every existing
     binding-number reference in the file to shift, expanding diff
     blast radius for no semantic gain. Adding to slot 4 keeps the
     diff narrow.
   - Falsifier: If the wasm target reports
     `max_storage_buffers_per_shader_stage < 5` on `@group(1)`, the
     binding-4 add would fail validation. Per the diagnostics
     snapshot the wasm cap is at least 8, so this is safe.

3. **Decision: Delete `GpuBoundQueueInfo` (step 3 option b) rather
   than keeping it as a documentation marker.**
   - Rejected: Keeping the struct for "compatibility." It's
     internal to the construction module; no external consumer
     references it. A dead type is just clutter.
   - Falsifier: If a separate workstream (not in this fix) consumes
     `GpuBoundQueueInfo`, deletion would break it. Verified by
     grepping for the symbol — only internal references.

4. **Decision: Keep probe-1B instrumentation in place through
   verification, remove in a follow-up patch.**
   - Rejected: Removing the probe as part of THIS fix to keep
     the PR clean. The diagnostic value of keeping it through
     verification (observing whether the prepare-call sequence
     normalises on web) outweighs the minor diff inflation.
   - Falsifier: None — the probe is observation-only.

5. **Decision: Verify Shape B on native unit-test BEFORE attempting
   the web gate.**
   - Rejected: Going straight to the web gate. The native unit
     test is a 30s feedback loop; the web gate is a 240s feedback
     loop. If the WGSL refactor has a binding-number typo, catching
     it on native saves 8 minutes per iteration.

6. **Decision: Treat the C# `BoundQueueInfo` packed struct as a
   non-binding reference, not a faithful-port constraint.**
   - Rejected: Maintaining the packed struct for faithful-port
     compliance. The struct's only purpose in C# is D3D11 buffer
     layout convenience; the algorithm doesn't depend on the two
     fields being adjacent. The Bevy port already diverges on
     several similar layout choices (e.g. the chunks-buffer
     migration from a 3D texture to a flat buffer for WebGPU
     compatibility, `bounds_calc.wgsl:96`); splitting
     `bound_queue_info` is consistent with that precedent.
   - **User-approval flag:** Per the project's faithful-port rule
     in `CLAUDE.md` ("Deliberate divergences require explicit user
     approval"), this divergence merits a one-line surface to the
     user at the synthesis pause. The recommended language: "Fix
     splits `bound_queue_info` (start, size) struct into two flat
     buffers to match the working `bound_group_masks` Tint lowering
     pattern; same class of WebGPU-port-driven layout divergence
     as the existing chunks-buffer split."

7. **Decision: Skip Shape G (queue rebuild) entirely.**
   - Rejected: Adding a queue-rebuild kernel. Large algorithmic
     surface; major faithful-port divergence; would require user
     approval AND extensive new unit-test surface. Shape B is
     architecturally cheaper and more targeted.
   - Falsifier: If Shape B+A both fail, Shape G becomes a
     candidate for re-orchestration.

8. **Decision: Do NOT propose adding `volatile` or `coherent`-style
   workarounds at the WGSL source level.**
   - Rejected: Trying `var<storage, read_write, atomic>` or similar
     non-standard attribute syntax. WGSL does not expose these
     decorations to user code (per `01-diagnostics-design.md` §B.2:
     "WGSL §14.5 ... relaxed/acquire/release decorations are
     expressible at SPIR-V/HLSL/MSL levels but WGSL does NOT
     surface them to user code"). Any attempt would either fail
     validation or be a no-op.

9. **Decision: Run the parity gate 3× as the stability bar, not 5×
   or 10×.**
   - Rejected: A larger N for stronger statistical confidence.
     The brief specifies 3× as the deliverable. 3 consecutive
     PASS results at SSIM ≥ 0.91 already rules out the
     historically-observed 0.69–0.94 variance window
     (P(broken passing 3 times) is negligible). Higher N adds
     time without commensurate confidence.

10. **Decision: Do NOT remove the probe-1B `@group(3)` binding as
    part of this fix.**
    - Rejected: Bundling the probe-1B teardown with the fix. The
      probe lives on its own bind group and pipeline-layout slot
      (`@group(3) @binding(0)`); it does not interact with the
      `@group(1)` changes in this design. Bundling teardown
      creates more diff and more places for a slip-up to break
      the probe before its observational role is complete.

11. **Decision: The fix is wasm32-AND-native-touching, not
    cfg-gated.**
    - Rejected: Cfg-gating the WGSL changes via Bevy's shader
      preprocessor `#ifdef`. Two reasons: (a) Bevy 0.19's shader
      `#ifdef` machinery is Bevy-internal-only, not a portable
      mechanism we can rely on for cross-target divergence; (b)
      the layout change is benign on native — `array<atomic<u32>>`
      lowers identically through naga as it does through Tint
      when the field was already-atomic; native should be
      bit-identical. The unit-test gate (step 7) is the safety
      net.
    - **Risk acknowledgement:** This is the one decision that
      violates the brief's "stay within the wasm-only code path
      where possible" guidance. Justification: WGSL has no
      cfg-gate mechanism that survives the asset pipeline; the
      WGSL changes MUST be unconditional. The Rust changes (steps
      2-5) are also unconditional because they have to match the
      WGSL. Native correctness is guarded by the unit-test gate.

## Assumptions made

1. **`array<atomic<u32>>` is correctly Tint-lowered with coherence
   decorations on Dawn.** This is the load-bearing assumption — it's
   what makes Shape B work. Evidence: `bound_group_masks` is declared
   that way (`bounds_calc.wgsl:107`) and the probe-1B data shows
   masks ARE consumed correctly cross-pass on web. Falsified if
   post-fix SSIM stays in the broken cluster.

2. **The wasm-side `max_storage_buffers_per_shader_stage` is ≥ 5 for
   `@group(1)`.** Per the device-snapshot data (referenced in
   `03-diagnosis.md` Section A), web's cap is at least 8. Adding one
   binding to `@group(1)` (4 → 5) is safe. Falsifier: pipeline
   creation fails on web with a validation error mentioning the
   storage-buffer cap.

3. **The native unit-test gate covers the algorithmic semantics of
   the layout split.** If `bounds_calc_convergence_matches_cpu_oracle`
   passes after step 5, native correctness is preserved — that test
   reads back the chunks buffer and asserts chunk-by-chunk match to
   the CPU oracle, which is the strongest possible end-to-end check
   short of the full e2e gate. Falsifier: the test passes on native
   but the native e2e gate (step 8) produces a different image.

4. **The probe-1B instrumentation does not interact with the
   `@group(1)` layout changes.** The probe is at `@group(3)
   @binding(0)`, on a separate bind group; it is only bound on the
   prepare pipeline. Changes to `@group(1)` should not affect it.
   Falsifier: pipeline validation error mentioning the probe binding,
   or the probe output becoming malformed post-fix.

5. **The render-graph node's encoder is the right place to drive
   regime-2 (no change there from the current code).** Step 11's
   fallback DOES require routing around the render-graph encoder
   for wasm; that's accepted scope-creep IF step 11 fires.

6. **`bound_group_count_of` returns a non-zero value for the Oasis
   test world.** Per `bounds_calc.rs:395-403` the function requires
   each axis divisible by 4 AND chunk count ≥ 64. Oasis's 23×8×21
   chunks fails the divisibility check on the X (23) and Z (21)
   axes. **Need to verify** — if Oasis-on-web returns 0 from
   `bound_group_count_of`, regime-2 is dormant entirely and the bug
   is something else entirely. Falsifier: probe-1B's
   `[probe1-call-meta] DRAIN COMPLETE: entries_emitted=N` with N=0.
   The probe-1B data shows N=200+ on web, so regime-2 IS running on
   Oasis. The Oasis chunk dimensions used in the test world must
   therefore differ from the 23×8×21 quoted in `03-diagnosis.md`
   §I — likely the Oasis fixture uses padded dimensions
   (e.g., 24×8×24 chunks). The fix doesn't depend on this assumption
   in itself; flagged for the impl agent to confirm dimensions in
   the probe output.

7. **The probe-1B GitHub diff at `04-probe1-impl.md` is
   applied in this worktree.** The diff lists 4 source files with
   probe additions. Step 5's fixture changes must coexist with the
   probe-1B fixture changes (the W3Fixture already has
   `prepare_probe_history` + `probe_bg` fields per
   `04-probe1-impl.md:107`). The impl agent must not regress the
   probe-1B changes when applying step 5. Falsifier: probe-1B output
   stops appearing in the web log after the fix.

8. **Bevy 0.19's `BindGroupLayoutEntries::sequential` accepts a
   tuple of 5 entries.** The current usage at
   `bounds_calc.rs:96-110` uses 4. Bevy's API supports arbitrary
   tuple arities up to ~16 via macro expansion. Falsifier: trait
   bound failure at compile time; if so, switch to
   `BindGroupLayoutEntries::with_indices` or array form.

9. **The native baseline `vox_horizon_native.png` is committed and
   stable.** The native gate must produce a comparable image; if the
   layout change shifts the native render at all (it shouldn't —
   the algorithm is identical), the cross-target SSIM compare
   floor would shift. Falsifier: native SSIM-to-prior-baseline
   < 0.99 after the fix.

10. **The faithful-port rule's "explicit user approval" requirement
    is satisfied by surfacing decision #6 above to the user at the
    orchestrator's synthesis pause.** The orchestrator (not the
    impl agent) is responsible for that user-touch.

## Open questions for the orchestrator / user

1. **Faithful-port divergence approval.** Decision #6 above splits
   the C# `BoundQueueInfo` struct into two Bevy-side flat buffers.
   This is a layout-only divergence (algorithm unchanged) and is in
   the same class as the existing chunks-buffer split. The
   orchestrator should surface this for explicit user approval per
   the CLAUDE.md faithful-port rule before the impl phase begins.
   Expected user reaction: approve, given it's the architecturally
   cheapest fix for a WebGPU-port correctness bug.

2. **Step 11 fallback authorisation.** If Shape B alone is
   insufficient (step 10 fails), the impl agent should layer in
   Shape A (per-round encoder+submit on wasm32, step 11). Does the
   user want the impl agent to apply step 11 automatically on
   failure, or pause for re-orchestration? Recommended: apply
   automatically (the layered Shape B+A is documented in this
   design; it's a single straightforward additional change with
   bounded risk). Re-orchestration is only needed if step 10 still
   fails after step 11.
