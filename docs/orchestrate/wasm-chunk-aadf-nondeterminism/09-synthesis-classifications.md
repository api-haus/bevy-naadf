# Synthesis + ranked bug classifications

> Read-only synthesis pass. No code edits, no builds, no e2e runs. Cites
> `00-handoff-verbatim.md` ... `08-probe2-impl.md` + `IMPLEMENTORS_SHARED.md`
> + verified source at `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl`,
> `crates/bevy_naadf/src/assets/shaders/world_data.wgsl`,
> `crates/bevy_naadf/src/assets/shaders/chunk_calc.wgsl`,
> `crates/bevy_naadf/src/render/construction/bounds_calc.rs`,
> `crates/bevy_naadf/src/render/construction/config.rs`.

## A — Situation summary

The bug as it stands today: the voxel raymarcher renders correctly on native
Vulkan but on WASM/Chrome/Dawn produces a partial scene whose SSIM against
the native reference varies run-to-run inside a tight, deterministic cluster
of 0.69-0.81 (well below the user-pinned floor of 0.91 at
`e2e/tests/vox-horizon-parity.spec.ts:65`). Probe-1B
(`04-probe1-impl.md`) showed that within the W3 regime-2 bounds-compute
loop, web's `prepare_group_bounds` only ever observes the `size0_*` queues
populated and drains them linearly 32768->4096 over 8 calls each
(`04-probe1-impl.md` table around line 442-451); native walks the full
size x axis ladder once with `found_size = 32768` and converges in 93
calls. The per-call probe values are byte-identical across 3 web runs;
only the total call count varies (200 / 205 / 210). Across the entire
investigation:
- **H1** (Tint omits `Coherent` / `MakeAvailable` SPIR-V decorations on
  `bound_queue_info`) was empirically falsified by Shape B
  (`06-fix-impl.md`): refactoring `bound_queue_info` into
  `bound_queue_starts: array<u32>` + `bound_queue_sizes:
  array<atomic<u32>>` left the probe pattern byte-identical and SSIM at
  0.693 on the only run executed before stop-on-fail.
- **M1** (Dawn does not insert an end-of-encoder availability barrier, so
  the last compute's `atomicAdd` is not flushed across `queue.submit`) was
  empirically falsified by probe-2 (`08-probe2-impl.md`): an
  `end_of_encoder_noop` compute pass that `atomicLoad`+`atomicStore`-s
  `bound_queue_sizes[0]` AFTER `compute_group_bounds` and BEFORE
  `queue.submit` left both the probe pattern and SSIM (0.693, 0.695,
  0.791) unchanged.
- **Two consolidated implementor dispatches** in
  `IMPLEMENTORS_SHARED.md` tried 9 more variants: (a) revert wasm to
  one-encoder-per-frame (mirror native), (b) raise `n_bounds_rounds`
  to 40 (force algorithmic convergence), (c) add `chunks[0]` touch to
  the noop, (d) `copy_buffer_to_buffer(chunks, indirect, 4)`, (e)
  full-size `copy_buffer_to_buffer(chunks, scratch, 16 MiB)` both
  directions between rounds, (f) flip the renderer's chunks binding
  from read to read_write (`world_data.wgsl:60`). Every one produced
  a result in the same 0.69-0.81 statistical cluster. Three
  individual runs across all these attempts happened to land
  >=0.91 ("lucky") — one in iter-1 H1, one in iter-4 H4, one in
  iter-2-4 H4-redo — but no intervention produced 3/3 PASS.

**The central tractability argument.** The handoff (line 16) and
`IMPLEMENTORS_SHARED.md` (iter-4 + iter-2-4) both document that web HAS
been observed to converge to native-equivalent quality on at least
one run. iter-4's PASS run produced an image visually nearly identical
to native (`IMPLEMENTORS_SHARED.md:244-245`). The system CAN produce
the correct output; the question is what conditions make that occur,
and what would force them deterministically.

A second piece of evidence reframes the bug entirely. iter-2-4 added a
chunks readback probe (the `aadf-probe2` AADF dump) at a known chunk
position post-convergence. On native that chunk reads
`chunk_aadf=[mx=4 px=4 my=3 py=3 mz=3 pz=3]` (multi-axis,
multi-round expansion). On web (FAIL runs) it reads
`chunk_aadf=[0,0,0,0,1,1]` — only the Z axis has been expanded, and
only by 1. iter-2-1 baseline matches this. This holds **even when**
iter-2 of dispatch-1 raised `n_bounds_rounds` to 40 and the probe-1B
data confirmed the algorithm reached all 32 bound-size levels
(`IMPLEMENTORS_SHARED.md:255`). So the bound-queue convergence
(which probe-1B was instrumented for) is decoupled from the
user-visible symptom; the bug now manifests in the
`chunks[chunk_idx]` writes from `compute_group_bounds` not being
visible to subsequent compute rounds' reads of the same chunks.

## B — The phenomenology

### B.1 Native-vs-web differences observed

From `02-diagnostics-impl.md`'s diag-compare output: **95 total
divergences, 84 unexpected, 6 load-bearing**.

Load-bearing six (lines 83-88, 96-101 in the comparison output):
1. `adapter_features.only_in_native` (and the same set under
   `device_features.only_in_native`): 53 features present on native that
   are absent on web. The ones I treat as relevant for the rest of the
   classifications:
   - `memory-decoration-coherent`, `memory-decoration-volatile` — the
     WGSL escape hatches that would let the shader request coherent
     storage semantics. Native exposes both; web exposes neither.
   - `mappable-primary-buffers` — native-only buffer mapping class that
     bypasses the `HOST_VISIBLE` -> `DEVICE_LOCAL` staging pattern.
   - `subgroup`, `subgroup-barrier`, `subgroup-vertex` — present on
     native, absent on web (orthogonal — no shader uses subgroup
     intrinsics per the grep in `03-diagnosis.md` Section B).
2. `adapter_limits.max_buffer_size`: `1 TiB` on native vs **`4 GiB-4`** on
   web (>1000x divergence).
3. `adapter_limits.max_storage_buffers_per_shader_stage`: `524288` on
   native vs **`16`** on web (>32000x divergence). The mirrored
   `device_*` fields are identical.

Other suggestive divergences (`02-diagnostics-impl.md:90-178`):
- `adapter_info.subgroup_min_size`: native = 32, web = 4. Subgroup
  variability on web spans 4..128 vs native's fixed 32. Construction
  shaders don't use subgroups, but `compute_group_bounds` declares
  `@workgroup_size(4, 4, 4)` = 64 threads, which on a web subgroup of 4
  would be 16 subgroups per workgroup — semantically equivalent under
  WGSL's workgroup-scope `workgroupBarrier()`, but a divergence the
  driver's compute scheduler may exploit.
- `adapter_limits.max_bind_groups`: 8 on native vs **4** on web. The
  prepare pipeline currently uses exactly 4 groups (with probe-1B's
  `@group(3)`); at the cap.
- `adapter_limits.max_dynamic_storage_buffers_per_pipeline_layout`: 16 on
  native vs 8 on web. Not exercised (no dynamic offsets used per
  `03-diagnosis.md` Section A).
- `adapter_limits.max_texture_dimension_3d`: 16384 on native vs **2048**
  on web. Triggered the chunks-as-texture -> chunks-as-buffer migration
  pre-handoff (`world_data.wgsl:54-59`).
- `adapter_limits.min_storage_buffer_offset_alignment`: 32 on native vs
  **256** on web (8x tighter). Moot for this codebase (no dynamic
  offsets).
- `adapter_features.only_in_native: shader-float32-atomic`,
  `shader-int64-atomic-*` — present on native, absent on web. Not used by
  any construction shader (per `03-diagnosis.md` Section A).

### B.2 Random-chance-web behavior

The handoff's central anomaly (line 16-17 + 115-117): on ONE recorded run,
the user observed ray reach grow from 50% to 100% over ~1 second, then
the same test config did not reproduce that growth afterward.
`IMPLEMENTORS_SHARED.md` reports three "lucky" PASS runs across the 9
attempted interventions (iter-1 run 2, iter-4 run 3, iter-2-4 run 1 at
0.810 — strictly speaking marginal, not PASS, but markedly higher than
its sibling runs at 0.693). iter-4's PASS run "passed with image
visually nearly identical to native (dense buildings, ocean horizon
line)" (line 245). The interventions do not deterministically prevent
or guarantee the lucky outcome.

The right framing: the algorithm CAN reach native-equivalent state on
web; the path it takes through state-space is non-deterministic, and
only specific paths converge in the time available. The state-space
trajectory is gated by some condition (cross-pass memory visibility on
`chunks[]`) which on most runs collapses to the bug and on rare runs
collapses to the working configuration.

### B.3 The same-slot-visible vs different-slot-invisible asymmetry on web

From the probe-1B table (`04-probe1-impl.md:442-451`):

| call_idx | native | web (3 runs) |
|---|---|---|
| 0 | qi=size0_ax0 sz=32768 | qi=size0_ax0 sz=32768 |
| 1 | qi=size0_ax1 sz=32768 | qi=size0_ax0 sz=28672 |
| 2 | qi=size0_ax2 sz=32768 | qi=size0_ax0 sz=24576 |
| 3 | qi=size1_ax0 sz=32768 | qi=size0_ax0 sz=20480 |
| ... | ... | ... (drains size0_ax0 to 0) |
| 8 | qi=size2_ax2 sz=32768 | qi=size0_ax1 sz=32768 |

The pattern: web's `prepare_group_bounds` IS observing the
`atomicStore(&bound_queue_sizes[size0_ax0], found_size - group_amount)`
written by the immediately-prior `prepare_group_bounds` call in the
same slot (the linear 32768 -> 28672 -> ... -> 4096 -> 0 drain).
What it is NOT observing is the `atomicAdd(&bound_queue_sizes[qi'], 1u)`
written by `compute_group_bounds` to OTHER slots (`bounds_calc.wgsl:532`
in the verified source — the re-enqueue site that increments the next
bound-size queue). Same buffer, two different writers, different
observability.

This asymmetry persists across every intervention tried. Shape B (top-
level `array<atomic<u32>>`) did not change it. M1 noop did not change
it. One-encoder-per-frame did not change it. Per-round-encoder+submit
did not change it. `copy_buffer_to_buffer` of `chunks` at 4 B did not
change it; at 16 MiB did not change it either.

The dispatch-2 chunks-probe finding REFRAMES the asymmetry: it is not
really about `bound_queue_sizes` at all. The bound-queue asymmetry was
a downstream symptom of the same root cause that produces the chunks
asymmetry. When `IMPLEMENTORS_SHARED.md` iter-2 raised `n_bounds_rounds`
to 40 and probe-1B confirmed the bound queues reach every level, the
chunks state was still wrong — meaning all the energy spent making the
bound-queue chain visible was wasted; the rendered output is governed
by `chunks[]` cross-pass write visibility, not by bound-queue
convergence. **The same-slot-visible vs different-slot-invisible
asymmetry applies to `chunks[]` directly:** each round's
`compute_group_bounds` reads `chunks[neighbour_idx].x` (line 252 in the
verified source) and `chunks[chunk_idx]` (line 499) and writes
`chunks[chunk_idx] = vec2<u32>(cur_chunk, entity_y)` (line 538). The
read on the next round of the SAME chunk_idx sees ... something — what
exactly is not directly probed (the AADF probe shows the
post-convergence value, not the per-round transition values).

## C — Ranked classifications

### Classification 1: Cross-workgroup cross-pass write visibility for non-atomic storage buffers is broken in Dawn / Chrome WebGPU for this access shape

- **Mechanism category:** A non-atomic storage-buffer write performed by
  one workgroup in compute pass N is not made visible to a non-atomic
  read of the same buffer by a different workgroup in compute pass N+1,
  even when both passes are in the same encoder and the inter-pass usage
  is `STORAGE_WRITE` -> `STORAGE_READ` on the same binding. The
  intra-encoder `vkCmdPipelineBarrier(SHADER_WRITE -> SHADER_READ)` that
  WebGPU spec §3.4.4 and Dawn's PassResourceUsageTracker
  (`07-diagnosis-round2.md` Section D.1) are documented to insert
  apparently either is not inserted (Dawn driver bug for this access
  shape), or is inserted but does not provide a memory-availability
  operation strong enough to flush compute-shader writes out of L1 /
  private caches into a state visible to a different workgroup in the
  next dispatch. The non-determinism enters because the convergence
  rate depends on driver-side batched-flush timing — sometimes the
  hardware happens to flush before screenshot, sometimes not.
- **Specific divergences cited:** (a) `memory-decoration-coherent`
  + `memory-decoration-volatile` present on native, absent on web
  (`02-diagnostics-impl.md` lines 86, 99). These are the WGSL escape
  hatches that would force the SPIR-V translator to emit `Coherent` on
  the storage-buffer access; their absence on the web feature set means
  Tint's lowering of `chunks: array<vec2<u32>>` (`world_data.wgsl:60`)
  CANNOT carry that decoration even if the application requested it.
  (b) `mappable-primary-buffers` absent on web — Dawn's storage buffers
  are `DEVICE_LOCAL`-only, so any host-visible coherence path is
  unavailable. (c) subgroup-size variability (`subgroup_min_size` 4 vs
  native's 32 — `02-diagnostics-impl.md` line 90) — `compute_group_bounds`
  declares `@workgroup_size(4, 4, 4)` (`bounds_calc.wgsl:449`), so on
  web a 4 thread subgroup means 16 subgroups per workgroup, and the
  cache-line size that Dawn chooses to operate on may differ from
  native's 2-subgroups-per-workgroup arithmetic. Per-subgroup caching
  granularity changes which writes propagate together.
- **Why this category fits the empirical data:**
  - The same-slot-visible vs different-slot-invisible asymmetry: prepare's
    `atomicStore` to `bound_queue_sizes[size0_ax0]` is followed,
    in-pass-and-same-buffer, by another `atomicStore`/`atomicLoad` in
    the next round's prepare (same thread chain because prepare is
    `@workgroup_size(1,1,1)` -- single thread per dispatch). Within
    the same compute pass and same workgroup, intra-workgroup memory
    coherence works correctly on every modern GPU. Compute's
    `atomicAdd` to `bound_queue_sizes[qi']` is by a DIFFERENT workgroup
    (one of the 4096 `compute_group_bounds` workgroups) and the next
    prepare reads it from yet another workgroup-instance — that is the
    cross-workgroup case where the cache-coherence assumption breaks
    down on the web's relaxed memory model.
  - The byte-identical probe-1B pattern across 6+ web runs: the bug is
    structural, not a race per-write. Every web run sees the SAME drain
    pattern because every web run experiences the SAME cross-workgroup
    invisibility — only the timing of when the renderer captures the
    state varies (and that timing is hardware-clock-deterministic enough
    that 200-215 calls is a narrow window).
  - The "lucky 50->100% run" anomaly: a global cache flush from
    Dawn's batched-flush threshold (B4 in `IMPLEMENTORS_SHARED.md` line
    380), a GPU warmup tip-over (B3), or coincidental memory-bus
    contention from a concurrent JS / browser thread serving the same
    cache line, momentarily forces the GPU into a state where every
    prior compute write was actually globally visible. On that run the
    algorithm runs as native; the lucky window persists until the next
    cache eviction.
- **Why prior falsifications (H1, M1, bailed-impl hypotheses) don't rule
  this out:** H1 (Tint atomic-decoration omission) was tested on
  `bound_queue_sizes` only and was anchored on the wrong field — the
  user-visible bug now appears to be cross-workgroup visibility on the
  non-atomic `chunks[]` buffer, which the WGSL `atomic<u32>` shape
  doesn't apply to at all. M1 (end-of-encoder availability) was tested
  with a no-op that touched `bound_queue_sizes` ONLY (not `chunks[]`);
  even iter-3's attempt to add `chunks[0]` to the noop (single slot, by
  a single thread of a single workgroup) is structurally not the same
  as the failing case (cross-workgroup writes from 4096 workgroups
  each writing a different `chunk_idx`). Both falsifications targeted
  asymmetries that aren't the load-bearing one.
- **Concrete next-probe-or-fix shape that would CONFIRM this
  classification:** Instrument `chunks[]` reads and writes
  per-round in `compute_group_bounds` (write the prior-round
  `cur_chunk_load` and the new `cur_chunk` to a sidecar history
  buffer keyed by `(round_idx, workgroup_id, local_index)`).
  Read back at end-of-gate and compare native vs web. If web's
  round-N read of a `chunk_idx` returns a value that does NOT
  match any prior round's write by ANY workgroup, the
  classification is confirmed. The fix shape (out of scope for
  this synthesis) is to either upgrade `chunks[]` to
  `array<atomic<u32>>` and pay the atomic cost, or insert an
  explicit cross-encoder buffer-to-buffer copy through a typed
  view that triggers a stronger Dawn barrier path.
- **What CONFIRMING evidence would look like:** Per-round chunks
  history shows native rounds reading evolving non-zero values
  (multi-round expansion) while web rounds read all-zero or
  same-as-initial values most rounds. SSIM stays at the 0.69-0.81
  cluster.
- **What REFUTING evidence would look like:** Web's per-round
  chunks history matches native byte-for-byte while the final SSIM
  still disagrees. (Would push toward Classification 3 or 5.)
- **Confidence given the evidence base:** HIGH. The dispatch-2
  baseline AADF probe (`IMPLEMENTORS_SHARED.md:413-417`)
  observes web's final chunk reads at exactly the "only the
  last-processed axis got expanded" shape (`[0,0,0,0,1,1]`), which
  is the symptom expected if each round overwrites the last
  with stale-read-then-modify-then-write. That is the strongest
  single piece of evidence in the whole trace.
- **Cost to investigate further:** SMALL for the probe (one
  shader-side history buffer + readback, identical pattern to
  probe-1B). MEDIUM-LARGE for the fix (atomicising `chunks[]`
  ripples through `chunk_calc.wgsl`, `world_change.wgsl`,
  `world_data.wgsl`, `entity_update.wgsl`, every Rust layout, and
  every test; per `IMPLEMENTORS_SHARED.md:549` is "~3 days" effort
  per the second implementor's estimate.)

### Classification 2: Tint's WGSL->SPIR-V lowering on Chrome's Dawn emits relaxed-ordering atomics with no `MemoryScope::QueueFamily` decoration, and that the WGSL spec's nominal "device scope" for storage atomics is not honoured on the actual lowering — for this specific access pattern (cross-encoder cross-workgroup atomicAdd)

- **Mechanism category:** Tint translates `atomicAdd` /
  `atomicStore` / `atomicLoad` on `array<atomic<u32>>` to SPIR-V
  `OpAtomicIAdd` etc. with whatever default memory-scope and
  semantics it picks. WGSL §14.5
  (`01-diagnostics-design.md` Section B.2) and gpuweb #3935
  (`07-diagnosis-round2.md` Section C.2) say storage atomics
  should carry `MemoryScope::QueueFamily` and acquire-release
  semantics. If Tint instead emits `MemoryScope::Workgroup`
  (so that the synchronization only applies inside the
  workgroup) or `MemorySemantics::Relaxed`, then any cross-
  workgroup atomic read on the same buffer can return a stale
  value indefinitely.
- **Specific divergences cited:** Same as Classification 1
  on `memory-decoration-coherent`. The native Vulkan adapter
  through naga produces SPIR-V with the required decorations;
  Dawn's Tint does not (or does so inconsistently for the
  multi-writer-then-other-pass-reader case).
- **Why this category fits the empirical data:**
  - Same-slot-visible vs different-slot-invisible asymmetry: only
    partially fits — see Classification 1's explanation. A pure
    SPIR-V scope omission would break ALL atomic visibility,
    including prepare's same-slot reads (which work).
    `07-diagnosis-round2.md` Section E.2 explicitly flags this as a
    weakness of the M2 hypothesis.
  - The byte-identical web probe-1B pattern: consistent with
    structural relaxed-ordering — every run gets the same
    "writes are visible to writers in same dispatch, invisible
    to readers in next dispatch" behaviour.
  - The "lucky run" anomaly: if Tint emits relaxed-ordering
    atomics, the hardware MAY still happen to flush in the
    right order on a lucky scheduling alignment. Unlikely
    cause, but not zero.
- **Why prior falsifications don't rule this out:** Shape B
  (top-level `array<atomic<u32>>`) was the SAME lowering shape
  Tint was already using internally for the struct-with-atomic
  field; Tint may emit the same SPIR-V regardless of WGSL surface
  syntax. M1 noop addressed the cross-encoder barrier path, not
  the per-atomic SPIR-V decoration. None of the dispatch-2
  attempts touched the SPIR-V lowering shape.
- **Concrete next-probe-or-fix shape that would CONFIRM this:**
  Use `wgsl-analyzer` / `naga-cli` to translate the WGSL through
  both backends and diff the resulting SPIR-V on the relevant
  atomic ops. Or: file a Dawn bug with a minimal reproducer
  (two compute passes, atomic write from one, atomic read from
  the other, expect-vs-actual). Empirically, swap to a
  load-store loop with explicit `storageBarrier()` (workgroup
  scope, won't help cross-workgroup) and see if behaviour
  changes; if not, the WGSL primitive isn't the issue.
- **What CONFIRMING evidence would look like:** SPIR-V diff shows
  Tint emits `Workgroup`/`Relaxed` where naga emits
  `QueueFamily`/`AcquireRelease`. Filed Dawn bug acknowledged.
- **What REFUTING evidence would look like:** SPIR-V emissions
  match; per-atomic memory-scope is the same; the bug persists.
  (Pushes toward Classification 1 or 3.)
- **Confidence given the evidence base:** LOW-MEDIUM. The
  prepare-visible asymmetry actively weakens this; the dispatch-2
  chunks-write-visibility finding is on a NON-atomic buffer, so
  this classification can't explain the user-visible symptom
  on its own.
- **Cost to investigate further:** SMALL-MEDIUM (offline SPIR-V
  inspection via naga/tint CLIs, no e2e iteration loop).

### Classification 3: Dawn's PassResourceUsageTracker for the chunks-buffer access transitions (RW in W3 compute pass -> RO in renderer pass) silently drops or mis-flags the cross-system / cross-encoder dependency, so the Vulkan submission omits the SHADER_WRITE -> SHADER_READ barrier on the actual user buffer

- **Mechanism category:** WebGPU §3.4.4 says each dispatch is one
  usage scope; the spec assumes the implementation tracks
  resource-usage transitions and inserts appropriate Vulkan
  barriers. Dawn's
  `dawn/src/dawn/native/CommandBufferStateTracker.cpp` is the
  tracker. The tracker is documented as per-encoder
  (`07-diagnosis-round2.md` Section D.1). When the W3 system's
  encoder finishes and Bevy's main-render encoder begins (a
  different encoder), Dawn's tracker resets. The cross-encoder
  dependency on `chunks_buffer` (W3 writes via
  `@group(1) binding(0)` rw, renderer reads via `@group(0)
  binding(0)` read at `world_data.wgsl:60`) is supposed to be
  resolved at submit time by wgpu-core's higher-level state
  tracker — but Bevy's render-graph submits all command buffers
  in ONE `queue.submit([cb_sys0..cb_sysN])` call
  (`07-diagnosis-round2.md` Section A.2), and the cross-buffer
  dependency may not be re-derived by Dawn at submit-time.
- **Specific divergences cited:** `max_storage_buffers_per_shader_stage`
  524288 native vs 16 web — Dawn enforces stricter usage tracking
  because it has a smaller per-stage slot count. The
  cross-binding-usage-flip (write in pipeline A, read in pipeline
  B with different bind groups) may be a corner case that
  wgpu-core's web backend doesn't fully decorate before passing
  to Dawn. `mappable-primary-buffers` absent on web means there's
  no host-visible fallback that would force a `vkInvalidateMappedMemoryRanges`
  on the cross-encoder boundary.
- **Why this category fits the empirical data:**
  - Same-slot-visible vs different-slot-invisible asymmetry: only
    indirectly. The dispatch-2 H2 test
    (`IMPLEMENTORS_SHARED.md:442-456`) explicitly flipped the
    renderer's chunks binding to read_write, hoping to make the
    usage class identical and avoid the cross-class transition.
    That REGRESSED to `[0,0,0,0,0,0]` (3/3 web SSIM 0.69) — making
    things worse, not better. Read as: the read_write flip
    introduced new dependencies the tracker handled even more
    poorly. Weak support, but not a clean falsification.
  - The byte-identical probe-1B pattern: doesn't speak to this
    classification directly; the probe instruments a different
    buffer (`bound_queue_sizes`) on the same W3 encoder.
  - The "lucky run" anomaly: every once in a while, the
    submit-order timing relative to Chrome's compositor thread
    flushes Dawn's deferred buffer state, and the barrier that
    should always exist happens to be inserted by the deferred
    flush path. iter-4 in `IMPLEMENTORS_SHARED.md` documented an
    intervention (full-size `copy_buffer_to_buffer` on chunks
    between W3 rounds) that produced a single PASS run with the
    visually-correct image — that copy IS a TRANSFER stage barrier
    that Dawn definitely tracks. The fact that even THAT didn't
    produce 3/3 PASS is the strongest evidence that Dawn's
    cross-encoder tracking is the bottleneck.
- **Why prior falsifications don't rule this out:** Shape B and
  M1 noop both targeted intra-W3-encoder atomics. Classification 3
  is about the cross-encoder boundary between W3 and the renderer,
  which neither targeted. iter-2-2 (one-encoder-per-frame for
  wasm) put W3 in Bevy's main encoder, which SHOULD have given
  Dawn the in-encoder tracker context to insert the barrier;
  it didn't help (`IMPLEMENTORS_SHARED.md:434`). That weakens
  Classification 3 but doesn't refute it — the within-system
  tracker logic may also have the same per-binding-usage-mismatch
  bug as the cross-encoder logic.
- **Concrete next-probe-or-fix shape that would CONFIRM this:**
  Add a wasm-only `device.poll(Wait)` after W3's last submit
  (analogous to `MAP_READ` of a 4-byte slice of chunks). On wasm
  `device.poll(Wait)` is a no-op, but issuing a `map_async` on a
  tiny scratch buffer that received a `copy_buffer_to_buffer(chunks,
  scratch, 4)` would force Dawn's submit path to flush all pending
  writes to chunks before the next frame starts.
- **What CONFIRMING evidence would look like:** SSIM stabilises at
  >=0.91 in 3/3 runs after the explicit fence, regardless of
  whether anything inside the W3 dispatch was changed.
- **What REFUTING evidence would look like:** Explicit fence
  changes nothing.
- **Confidence given the evidence base:** MEDIUM. iter-4's single
  PASS run with full-buffer `copy_buffer_to_buffer` is consistent
  with this; that the same intervention didn't produce 3/3 PASS
  is consistent with the underlying issue being deeper than a
  single barrier insertion.
- **Cost to investigate further:** SMALL (one wasm-only system
  edit, one Playwright re-run). The fix shape is also small if
  confirmed.

### Classification 4: `compute_group_bounds`'s read-then-write on `chunks[chunk_idx]` is intrinsically a read-modify-write hazard across rounds, and Dawn's storage-buffer memory model exposes a long-standing race that native Vulkan happens to obscure with permissive cache behaviour

- **Mechanism category:** The shader at
  `bounds_calc.wgsl:499` (`let cur_chunk_full = chunks[chunk_idx];`)
  reads the chunk; line 538 writes back the modified value
  (`chunks[chunk_idx] = vec2<u32>(cur_chunk, entity_y);`). This
  is a non-atomic read-modify-write. Two different
  `compute_group_bounds` rounds, each processing a different
  (size_level, axis) but sharing chunks at the boundaries of
  group-aligned regions, will both perform this RMW on
  overlapping chunks. The neighbour read at
  `bounds_calc.wgsl:252` (`let neighbour_x =
  chunks[neighbour_idx].x;`) is the actual cross-axis dependency:
  the X-axis expansion needs the Y-axis expansion's previous
  output. If round N's neighbour read on web sees stale
  (pre-round-N-1) values consistently, only the LAST axis's
  expansion survives. That matches the dispatch-2 finding
  exactly (`IMPLEMENTORS_SHARED.md:416`: web AADF reads as
  `[0,0,0,0,1,1]` — only Z, the last-processed axis).
- **Specific divergences cited:** `memory-decoration-coherent`
  absent. `subgroup_min_size` 4 vs 32 — on web with 4-thread
  subgroups, the cache-line that holds `chunks[chunk_idx]` may
  be marked dirty per-subgroup, so 16 subgroups per workgroup
  each have their own potentially-stale view of a chunk that a
  different workgroup just wrote. Native's 32-thread subgroup
  ALWAYS spans the entire 64-thread `@workgroup_size(4,4,4)`
  workgroup with 2 subgroups, so the cache-line dirtying
  granularity is workgroup-level, giving the RMW pattern a
  much better chance of seeing fresh values intra-workgroup
  even when cross-workgroup visibility is delayed.
- **Why this category fits the empirical data:**
  - Same-slot-visible vs different-slot-invisible asymmetry:
    fits because prepare's same-slot atomic stores are
    intra-workgroup (single thread, `@workgroup_size(1,1,1)`);
    compute's writes to different slots are different workgroups.
    The asymmetry IS the workgroup-instance asymmetry.
  - Byte-identical probe-1B pattern across web runs: the race is
    structurally deterministic — the same thread scheduling
    sequence on Dawn produces the same observed values.
  - The "lucky run" anomaly: hardware happens to flush a
    specific cache-line set at the right moment, and one round's
    writes propagate before the next round's reads. On lucky
    runs this happens for enough chunks that the visual quality
    crosses the SSIM threshold.
- **Why prior falsifications don't rule this out:** Shape B
  (sizes split) was about `bound_queue_sizes`, not `chunks[]`.
  M1 noop touched `bound_queue_sizes`, not `chunks[]`.
  iter-2-4 full-buffer `copy_buffer_to_buffer` of `chunks[]`
  produced the highest SSIM (0.810) and the first multi-axis
  expansion observation (4/6 axes got 1 bit), STRONGEST partial
  confirmation; it did not produce 3/3 PASS because even with
  the TRANSFER barrier between rounds, Dawn may stack-up the
  next compute pass before the prior copy completes (chained
  intra-encoder copies + dispatches don't necessarily fence
  each other on Dawn).
- **Concrete next-probe-or-fix shape that would CONFIRM this:**
  Switch `chunks[]` to `array<atomic<u32>>` and replace the
  RMW at line 538 with `atomicOr(&chunks[chunk_idx],
  1u << bounds_location)` for the AADF bit set. This eliminates
  the read-modify-write hazard entirely. Per
  `IMPLEMENTORS_SHARED.md:549` (Option C), this is a "~3-day
  refactor across 4 shaders + their Rust bindings + their layout
  descriptors."
- **What CONFIRMING evidence would look like:** SSIM stabilises
  >=0.91 in 3/3 runs. Web AADF probe reads multi-axis
  expansion (`[4,4,3,3,3,3]` or similar) matching native.
- **What REFUTING evidence would look like:** Atomicising
  doesn't help. Pushes back toward Classification 1 (storage
  visibility is broken at a level below the atomic primitive).
- **Confidence given the evidence base:** HIGH. The dispatch-2
  AADF probe finding (`[0,0,0,0,1,1]` vs native's
  `[4,4,3,3,3,3]`) is exactly the symptom predicted by an
  RMW-overwrite race that loses all but the last round's
  contribution per chunk. iter-2-4 chunks-copy result (4/6 axes
  expanded by 1) is the partial-fix pattern predicted by
  forcing visibility between SOME rounds but not all rounds.
- **Cost to investigate further:** MEDIUM-LARGE (the refactor
  is bounded but spans multiple shaders).

### Classification 5: Renderer-side bind group caching or upload-path race on wasm exposes pre-W3 chunks state to the renderer; the bug is post-W3, in the renderer pipeline or material-data upload chain

- **Mechanism category:** The renderer's
  `world_data.wgsl:60` binds chunks as read-only. The bind group
  is built once in `prepare` (per
  `IMPLEMENTORS_SHARED.md:391`). If the bind group's chunks
  buffer handle is a CACHED reference to a buffer that gets
  re-allocated when W3's first compute pass needs to expand the
  buffer (unlikely — sizes are fixed), or if the renderer reads
  from a pre-update mirror of chunks (also unlikely; chunks
  binding handles point at the live GPU buffer), or if some
  upload-path on wasm is dropping bytes — the renderer sees
  stale or zeroed chunks. The "lucky run" is when, by browser
  timing, the renderer happens to bind the right buffer.
- **Specific divergences cited:** `max_buffer_size: 4 GiB-4`
  on web — a buffer near that cap may behave differently under
  Dawn's allocator. The Oasis world's `voxels` allocation can
  be near 2 GiB; `chunks` is small (~30 KiB) per the
  `03-diagnosis.md` assumption #3. So this is less plausible.
  `mappable-primary-buffers` absent — affects upload path, but
  `write_buffer` should still work.
- **Why this category fits the empirical data:**
  - Same-slot-visible vs different-slot-invisible asymmetry:
    doesn't speak to this directly; the asymmetry is about
    intra-W3 atomics, this is about post-W3 rendering.
  - The byte-identical probe-1B pattern: orthogonal — the probe
    runs in W3, not in the renderer.
  - The "lucky run" anomaly: a browser-timing race could produce
    intermittent correctness.
- **Why prior falsifications don't rule this out:** No
  intervention has directly tested the renderer's chunks-binding
  freshness. iter-2-3 in `IMPLEMENTORS_SHARED.md` flipped the
  renderer's chunks binding to read_write (line 442-456); that
  REGRESSED to all-zeros, which is consistent either with
  Classification 3 OR with Classification 5 (the read_write flip
  may have triggered a different upload path or bind-group
  rebuild).
- **Concrete next-probe-or-fix shape that would CONFIRM this:**
  Disable W3 entirely on wasm (set
  `cfg.gpu_construction_enabled = false`, or short-circuit the
  W3 system). If the rendered image is identical to "what the
  CPU mirror produced" but is STILL different from native, the
  bug is in the renderer / upload path, not in W3 at all.
  `IMPLEMENTORS_SHARED.md:318` lists this as next-dispatch
  recommendation #3 ("Bisect via... disable W3 on wasm").
- **What CONFIRMING evidence would look like:** Disabling W3
  doesn't help; image still wrong; bug is downstream.
- **What REFUTING evidence would look like:** Disabling W3
  produces correct output (within CPU-fallback expectations).
  This would refute Classification 5 and re-confirm the bug is
  in W3 (Classification 1-4).
- **Confidence given the evidence base:** LOW. The dispatch-2
  AADF probe finding (web reads `chunk_aadf=[0,0,0,0,1,1]` —
  a half-expanded value that COULD ONLY come from one round of
  W3 having run + written) directly contradicts pure
  renderer-side or upload-side theories. The chunks values web
  reads are partial-W3-output, not pre-W3 or post-W3-cached
  output.
- **Cost to investigate further:** SMALL (toggle one config
  bool, run gate 3x).

### Classification 6: The bug class is a stochastic chrome / dawn batched-flush threshold that has nothing to do with any particular access pattern; it is an emergent property of Dawn's submission queue depth and Chrome's compositor scheduling

- **Mechanism category:** Dawn batches GPU work and flushes it
  to the underlying Vulkan queue based on internal thresholds
  (e.g. N pending submits, M pending bytes, or scheduler tick
  boundaries). The renderer's screenshot happens at a fixed
  frame after `cpu_mirror_populated`
  (`mod.rs:3275-3565`); whether the W3 writes have crossed
  Dawn's flush threshold by that frame is a function of every
  factor in the system, INCLUDING JS thread load, browser
  compositor ticks, GPU process IPC latency, etc. The "lucky
  run" is when the threshold gets crossed sometime between
  W3's first dispatch and the screenshot. The deterministic
  cluster (0.69-0.81) reflects how many W3 rounds happened to
  flush; the "lucky PASS" reflects all W3 rounds happening to
  flush.
- **Specific divergences cited:** Not a single capability
  divergence; this classification is about scheduling, not
  capability. Indirectly supported by the absence of features
  like `mappable-primary-buffers` (no way to explicitly force a
  flush from the host).
- **Why this category fits the empirical data:**
  - Same-slot-visible vs different-slot-invisible asymmetry:
    explained as "prepare's same-slot writes happen to be in a
    cache-line that's already evicted by Dawn's flush, while
    compute's different-slot writes are not." Plausible but
    hand-wavy.
  - The byte-identical probe-1B pattern: the BOUND-QUEUE state
    is deterministic because the W3 algorithm's compute pattern
    deterministically queues work in the SAME order, AND Dawn's
    flush threshold is deterministic enough that the cumulative
    flushed-bytes count is the same each run.
  - The "lucky run" anomaly: directly explained as
    "occasionally the flush threshold crosses the W3 finish
    line before screenshot."
- **Why prior falsifications don't rule this out:** Every
  intervention in `IMPLEMENTORS_SHARED.md` produced the same
  statistical cluster — that's the smoking gun for "the bug is
  a scheduling property, not a code-path property". The
  interventions perturbed compute work, encoder boundaries,
  submit counts, and barrier types; the SSIM distribution
  barely shifted. If the bug were code-path-specific, you'd
  expect at least one intervention to land it.
- **Concrete next-probe-or-fix shape that would CONFIRM this:**
  Add an explicit `device.poll(Wait)` (or moral equivalent
  via `queue.on_submitted_work_done().await` on wasm) AT
  END-OF-W3 SYSTEM. This blocks the JS event loop until all
  prior submits are GPU-complete. If SSIM stabilises at >=0.91,
  the bug is "missing explicit fence" and the classification is
  confirmed. (NOTE: `device.poll` is a no-op on wasm per
  `01-diagnostics-design.md` §B.6, so this would need to be
  done via `queue.on_submitted_work_done()` awaited.)
- **What CONFIRMING evidence would look like:** SSIM stabilises
  >=0.91 3/3 after explicit submit-fence. Frame budget grows
  noticeably (we're waiting for GPU work).
- **What REFUTING evidence would look like:** No change in
  SSIM distribution.
- **Confidence given the evidence base:** MEDIUM-HIGH. The
  consistency of the failure across 9+ very different
  interventions is the strongest evidence for a scheduling
  cause. The dispatch-2 chunks-AADF probe finding constrains
  this classification: the chunk values web reads are
  partial-W3-output, which means W3's writes DID partially
  reach memory, just not fully. That's consistent with a
  flush-threshold-crossing-mid-W3 scenario.
- **Cost to investigate further:** SMALL.

## D — Cross-classification cumulative falsification map

Empirical findings as rows. Classifications 1-6 as columns. Cells:
**confirms** (the finding directly supports this classification);
**refutes** (the finding directly contradicts this classification);
**neutral** / orthogonal otherwise.

| Empirical finding | C1 (cross-pass non-atomic) | C2 (Tint SPIR-V scope) | C3 (Dawn cross-encoder tracker) | C4 (RMW hazard) | C5 (renderer/upload) | C6 (stochastic flush) |
|---|---|---|---|---|---|---|
| H1 falsified (Shape B sizes split → SSIM 0.693, probe pattern byte-identical) | neutral | refutes (same lowering shape didn't help) | neutral | refutes (atomicising sizes didn't help) | neutral | neutral |
| M1 falsified (end-of-encoder noop touching sizes → unchanged) | refutes-partially (intra-encoder barrier on sizes didn't help; but noop was sizes-only, not chunks) | refutes | refutes-partially (within-encoder fence didn't propagate) | neutral (didn't touch chunks RMW) | neutral | confirms (intervention didn't shift cluster) |
| Same-slot-visible vs different-slot-invisible asymmetry (bound_queue_sizes) | confirms (cross-workgroup is the broken case) | refutes-strongly (would break same-slot too) | neutral | confirms (different workgroups ↔ different slots) | refutes (orthogonal to renderer) | confirms-partially |
| Byte-identical probe-1B pattern across 3 web runs | confirms (structural bug, not race per-write) | confirms | confirms | confirms | refutes (would predict more variance) | confirms |
| `[0,0,0,0,1,1]` web AADF readback vs native `[4,4,3,3,3,3]` (iter-2-1) | confirms-strongly (only-last-round-survives RMW pattern) | refutes (chunks is non-atomic) | confirms-partially | confirms-strongly | refutes (renderer sees post-W3 partial state, so W3 ran) | confirms-partially |
| iter-2 raised n_bounds_rounds=40, queue reaches every level, SSIM unchanged | refutes-on-the-narrow-version (bound-queue convergence isn't the bottleneck) | refutes | neutral | confirms (more rounds = more RMW overwrites = no improvement) | neutral | confirms |
| iter-2-4 full-size chunks copy_buffer_to_buffer → SSIM 0.810 (highest), 4/6 axes expanded | confirms (transfer barrier partially fixes chunks visibility) | refutes | confirms (cross-encoder/cross-pass tracker is at fault) | confirms-strongly | refutes | refutes-partially (intervention DID shift quality) |
| iter-2-3 flipped renderer chunks to read_write → REGRESSED to all-zeros, SSIM 0.69 3/3 | neutral | refutes | refutes-partially (read_write flip should have removed cross-class transition) | refutes-partially | refutes-strongly (the read_write flip on renderer was direct test of upload/binding theory) | neutral |
| All 9+ interventions produce same 0.69-0.81 cluster | confirms (bug is structural, doesn't move) | confirms | confirms | confirms | refutes | confirms-strongly |
| ~1 in 3-9 runs achieves lucky PASS (the central tractability evidence) | confirms (cache-flush window) | confirms | confirms | confirms | confirms | confirms |

Net: **C1 and C4 are the strongest fit**, with C3 and C6 as legitimate
secondary explanations. C2 and C5 are mostly refuted but cannot be fully
ruled out without targeted probes.

## E — The "sometimes works" reconciliation

The cross-cutting question. Six possible mechanisms for why web
converged once and not other times, mapped to which classifications
each is consistent with:

1. **GPU warmup state (driver scheduling latency drops after first
   few seconds).** Consistent with C1 (cache state warmer = writes
   propagate sooner), C4 (RMW races resolve more deterministically
   with stable cache state), C6 (Dawn's flush thresholds may be
   based on time-since-last-flush which warms up). Mildly inconsistent
   with C2 (SPIR-V emission doesn't change at runtime), C3 (Dawn's
   tracker doesn't change behavior with warmup), C5 (bind-group
   freshness doesn't change with warmup).
2. **Memory page allocation luck (a specific page layout enables
   coherent visibility).** Consistent with C1 + C4 (cache-line
   alignment matters for cross-workgroup visibility), C6 (page
   allocation affects flush boundaries). Less so with C2, C3, C5.
3. **Concurrent JS thread work (frame budget delta that gives the
   GPU longer per round).** Consistent with C6 (more time-per-frame
   = more chance to hit flush threshold), C3 (more time = more
   chance for Dawn's tracker to materialize). Plausible for C1 / C4
   (more time = more chance for caches to settle). Unrelated to C2,
   C5.
4. **Cache state (cold-cache vs warm-cache visibility differs).**
   Consistent with C1 (warm cache = writes hit L2 faster), C4
   (cache-line ownership stable across cross-workgroup access).
   Mildly with C3 (Dawn may flush differently under cache pressure),
   C6. Unrelated to C2, C5.
5. **Driver scheduler quirk (Dawn's submission ordering occasionally
   synchronously flushes).** Consistent with C6 directly. Consistent
   with C3 (the scheduler-side flush DOES provide the missing
   barrier). Consistent with C1, C4 (flush is what propagates
   writes). Less so with C2, C5.
6. **WebGPU implementation race (Chrome's adapter implementation has
   internal non-determinism that occasionally happens to insert a
   barrier).** Consistent with C3 directly, C6, C1, C4. Less so
   with C2 (SPIR-V is deterministic), C5 (renderer-binding state
   is deterministic).

**The unifying explanation across the strongest classifications
(C1 + C3 + C4 + C6):** Web's GPU memory model intermittently
*flushes* pending compute writes to a state where subsequent
dispatches can read them, when conditions align — that flush is what
"sometimes works" looks like. The flush condition is some
combination of (cache-line ownership change, batched-flush threshold
crossing, Dawn's deferred state materialization, hardware scheduling
slack). On the unlucky common case, the flush doesn't happen before
the renderer captures the frame; on the lucky rare case, it does.

**The fix is whatever forces that condition deterministically.** Per
the classifications:
- C1/C4 fix: atomicise the `chunks[]` access pattern, replacing
  cross-workgroup RMW with atomic-Or; intra-cache-line atomicity
  forces the flush per-op.
- C3 fix: insert an explicit host-observable fence
  (`queue.on_submitted_work_done()`-equivalent on wasm) between W3's
  last submit and the renderer's first chunks-read.
- C6 fix: same as C3 — explicit fence.

The good news: C3 and C6 share the same fix, and that fix is the
SMALLEST intervention available. It should be the first thing tried.
If it works in 3/3, the bug is in the C3+C6 family. If it doesn't,
C1+C4 (atomicise chunks) becomes the next step.

## F — Synthesised verdict

The single best classification (combined C1 + C4) is the
**cross-workgroup cross-pass non-atomic storage visibility race on
`chunks[]`** — `compute_group_bounds`'s neighbour-read at line 252
and its read-modify-write at lines 499 + 538 do not reliably observe
the prior round's writes on web, with the asymmetry that
prepare's own-slot atomic visibility works because it is single-
threaded and intra-workgroup. The dispatch-2 AADF probe finding
(`[0,0,0,0,1,1]` vs native's `[4,4,3,3,3,3]`) is essentially a
direct measurement of the bug: only the last axis's expansion
survives because each round overwrites the previous round's chunks
with a stale-read + own-axis-modify + write-back. This is a
known-class GPU bug (cross-workgroup RMW without atomic), historically
masked on native Vulkan by aggressive driver-side cache coherence
that Dawn does not provide.

**Uncertainty:** there is a real chance the bug is partly
Classification 3 / 6 (the cross-encoder fence missing) on top of
Classification 1 / 4, in which case fixing only the RMW pattern
without also fixing the cross-encoder fence leaves residual
non-determinism. Conversely, if Classification 6 (stochastic flush)
is the dominant cause, atomicising `chunks[]` may have no effect at
all and the only fix is an explicit submit fence.

The user's framing ("if it works sometimes it can be fixed") is
correct, and the fix is one of {force the flush deterministically,
eliminate the read-modify-write hazard, or both}. The deliverable for
the next agent is not yet a fix; it is a probe that
distinguishes C3/C6 (cheap fence fix) from C1/C4 (larger refactor).

## G — Recommended next dispatch shape

Ranked by expected information-value-per-investigation-cost:

1. **Single-shot fence probe (LOW cost, high information value).**
   Dispatch a sub-agent that adds, AFTER the W3 system's last submit
   on wasm, a `queue.on_submitted_work_done()` awaited via a small
   integration (or its moral equivalent via a tiny
   `map_async`-on-scratch pattern), then re-runs the parity gate
   3x. If 3/3 SSIM >= 0.91, the bug is in the C3/C6 family and
   the fix is approximately one Bevy-system edit. If 3/3 SSIM
   stays in the broken cluster, the bug is in the C1/C4 family
   (intrinsic to the chunks access pattern) and the next dispatch
   needs to atomicise. This single probe cuts the classification
   space in half.

2. **Chunks-history-probe (MEDIUM cost, definitive information
   value).** Instrument `compute_group_bounds` to write per-round
   per-workgroup observed `chunks[chunk_idx]` BEFORE and AFTER its
   RMW into a sidecar history buffer; read back; compare native
   vs web. Direct measurement of cross-pass chunks visibility.
   Confirms or refutes C1/C4 with the same kind of evidence
   probe-1B gave for the bound-queue case. Higher cost because the
   sidecar buffer is large (one entry per workgroup per round per
   compute pass). Lower expected information than the fence probe
   only because the fence probe might also produce a working fix
   along the way.

3. **External-help dispatch: file a minimal-reproducer Dawn issue.**
   Build a standalone WGSL+wgpu reproducer (no Bevy, no NAADF) of
   the cross-pass cross-workgroup atomic-visibility-on-different-
   slot pattern (the same probe-1B asymmetry, but in a 30-line
   shader). File at https://crbug.com/dawn for Chrome team triage.
   Expected return time: weeks. The probe-1B byte-identical-pattern
   data is publication-ready evidence. Useful as a parallel track to
   the synthesis-fix; the project doesn't need to wait on Chrome
   for the actual fix.

The recommended first move is (1). It costs ~1 dispatch and binarily
discriminates between the cheap-fix family and the expensive-fix
family. The user can then make an informed call about whether to
ship the cheap fix or commit to the expensive refactor.

## H — Decisions made by this analyst

1. **Treated the `IMPLEMENTORS_SHARED.md` chunks-asymmetry finding
   as load-bearing.** Alternative: privilege the round-2 diagnosis's
   `bound_queue_sizes` analysis because it has more detailed code
   citations. Why this won: the dispatch-2 iter-2 finding empirically
   refuted the "bound-queue convergence is the bottleneck"
   framing — the algorithm reaches every queue level with
   `n_bounds_rounds=40` and SSIM is unchanged. The user-visible bug is
   downstream of bound-queue convergence; the chunks-AADF mismatch is
   the actual mismatch.

2. **Ranked Classification 1 + 4 as primary, not separate.** They
   share a mechanism (cross-workgroup non-atomic storage visibility);
   the C4 RMW framing is just C1 instantiated for the chunks-buffer
   shape. Alternative: rank them separately. Why this won: the user
   asked for categorical classifications, not over-fragmented
   sub-cases; merging keeps the count manageable.

3. **Kept Classification 6 (stochastic-flush) despite its hand-wavy
   nature.** Alternative: drop it as un-mechanistic. Why this won:
   the consistency-across-9-interventions evidence is the single
   strongest argument for it, and it shares a fix with Classification
   3 — both are addressed by an explicit fence. Worth keeping as a
   live possibility.

4. **Dropped the round-1 H2 (4096-cap-related) and H4
   (FP-evaluation) hypotheses from the synthesis.** Alternative: keep
   them as classifications 7-8. Why this won: H4 is essentially
   refuted by the integer-only nature of the construction shaders
   (`03-diagnosis.md` Hypothesis 4 self-falsifies on inspection).
   H2 is downstream of H1 by the original diagnosis (no independent
   mechanism); subsumed under C1.

5. **Did not propose specific code changes.** Per the brief.

## I — Assumptions made

The top-ranked classifications (C1 + C4) depend on:

1. **The dispatch-2 AADF probe reading (`[0,0,0,0,1,1]` for web,
   `[4,4,3,3,3,3]` for native) is at the same chunk position on both
   targets.** `IMPLEMENTORS_SHARED.md:414` quotes the chunk at
   `(242, 31, 219)` for both. If the probe samples different chunks
   per target, the comparison is meaningless. Assumed correct based
   on the implementor's report.

2. **`compute_group_bounds`'s `@workgroup_size(4, 4, 4)` is 64 threads
   per workgroup, NOT 4 + 4 + 4 = 12.** Verified via
   `bounds_calc.wgsl:449`.

3. **Bevy's `submit_pending_command_buffers` does in fact batch all
   systems' command buffers in one `queue.submit(...)` call on both
   native and wasm.** Verified by `07-diagnosis-round2.md` Section
   A.2 reading
   `bevy_core_pipeline-0.19.0-rc.1/src/schedule.rs:228-240`. If
   wasm uses a different path (the per-round-encoder+submit branch
   in `bounds_calc.rs:620-650` clearly does), the cross-system
   fence assumption in Classification 3 may not apply.

4. **Chrome's bundled Dawn on the user's CachyOS box runs on Vulkan
   (not OpenGL or SwiftShader).** Verified by
   `02-diagnostics-impl.md` snapshot showing
   `adapter_info.backend = "browserwebgpu"` with the Vulkan-equivalent
   limit profile (2 GiB-4 storage binding size).

5. **The renderer's `chunks` binding is read-only
   (`world_data.wgsl:60`).** Verified in source. iter-2-3's
   read_write flip + regression confirms this is the production
   state.

6. **Probe-1B's instrumentation does not perturb the bug.** Both
   implementors confirmed it doesn't; the post-Shape-B probe pattern
   is byte-identical to pre-Shape-B. Assumed-safe.

7. **The "lucky run" pass observations are real and not measurement
   artifacts.** The user observed one in the live build; the
   implementor observed one in iter-1 and another in iter-4. Three
   independent observations across different test invocations.

## J — Open questions for the orchestrator / user

None. The next dispatch can proceed without further user input.
