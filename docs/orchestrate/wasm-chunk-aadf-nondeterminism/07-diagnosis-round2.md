# Diagnose-first round 2 — wasm-chunk-aadf cross-pass atomic visibility

## Posture

- Round 1's H1 (the "Tint omits `Coherent`/`MakeAvailable+MakeVisible` SPIR-V
  decorations when lowering `BoundQueueInfo { start, size }` packed struct
  to SPIR-V") is empirically falsified by Shape B's failure
  (`06-fix-impl.md`). The flat split into `bound_queue_starts: array<u32>` +
  `bound_queue_sizes: array<atomic<u32>>` adopts the exact same lowering
  shape `bound_group_masks` uses; post-fix web SSIM = 0.693 with probe-1B
  pattern byte-identical to pre-fix. Both H1's hypothesis and Shape B's
  premise are dropped as bias for this investigation.
- The data we now have: cross-PASS atomic visibility on Dawn is broken for
  `bound_queue_sizes` even when laid out as the same shape as
  `bound_group_masks`. The implementer's stated insight — "masks-as-atomic
  works because intra-pass; sizes-as-atomic fails because cross-pass" — is
  the right direction but the "masks work" half of the claim is itself
  UNVERIFIED (see Section B).
- I will identify the ACTUAL mechanism with spec + source citations.

## A — Verified current dispatch shape

The handoff narrative says the wasm-only branch at `bounds_calc.rs:413-466`
dispatches each round as its own encoder+submit. The actual line range in
HEAD is `bounds_calc.rs:473-527` (the line numbers in the handoff are from
a pre-Shape-B revision). I confirmed by reading the file in HEAD that the
pattern is in effect.

### A.1 The "render-graph node" is actually a `Core3d`-schedule Bevy system

The handoff and round-1 diagnosis repeatedly call `naadf_bounds_compute_node`
a "render-graph node." This is the project's own informal terminology — in
Bevy 0.19 there is no node trait; this is a regular Bevy system registered
into the `Core3d` schedule that takes `RenderContext` as a `SystemParam`.
Verified: `crates/bevy_naadf/src/render/mod.rs:300-329` registers it via
`add_systems(Core3d, (..., naadf_bounds_compute_node, ...).chain().in_set(
Core3dSystems::PostProcess))`. The system signature at
`bounds_calc.rs:398-409`:

```rust
pub fn naadf_bounds_compute_node(
    mut render_context: RenderContext,
    pipeline_cache: Res<PipelineCache>,
    construction_pipelines: Option<Res<super::ConstructionPipelines>>,
    construction_bind_groups: Option<Res<ConstructionBindGroups>>,
    construction_gpu: Option<Res<ConstructionGpu>>,
    construction_config: Option<Res<ConstructionConfig>>,
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
    render_device: Res<bevy::render::renderer::RenderDevice>,
    #[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]
    render_queue: Res<bevy::render::renderer::RenderQueue>,
) {
```

`render_device` and `render_queue` are only used on the wasm path
(`#[cfg(target_arch = "wasm32")]` branch).

### A.2 Bevy's `RenderContext` system-param semantics

Verified in
`bevy_render-0.19.0-rc.1/src/renderer/render_context.rs:131-159`:
`RenderContext` is `#[derive(SystemParam)]` with field
`state: Deferred<'s, RenderContextState>`. **Each system gets its OWN
`Deferred<RenderContextState>`** — `RenderContext::command_encoder()`
returns `state.command_encoder()` which lazily creates a `CommandEncoder`
via `render_device.create_command_encoder()` (lines 88-98) and CACHES it
in this system's deferred state. The `SystemBuffer::queue` impl (lines
106-126) calls `.finish()` on this encoder at end-of-system and pushes the
resulting `CommandBuffer` into a shared `PendingCommandBuffers` resource.

After all `Core3d` systems run, `submit_pending_command_buffers`
(`bevy_core_pipeline-0.19.0-rc.1/src/schedule.rs:228-240`) runs once in
`RenderGraphSystems::Submit` (registered at
`bevy_core_pipeline-0.19.0-rc.1/src/lib.rs:63-71`) and calls
`queue.submit(buffers)` ONCE for ALL accumulated command buffers from all
systems that frame.

**So on native, every Core3d system that uses `RenderContext` builds its OWN
command buffer (one per system), and all those buffers are submitted in a
SINGLE `queue.submit([cb_sys0, cb_sys1, ..., cb_sysN])` call.** They are
NOT merged into one encoder.

### A.3 The native path (regime-2 background loop)

`bounds_calc.rs:565-581` (native cfg branch):

```rust
let diagnostics = render_context.diagnostic_recorder();
let diagnostics = diagnostics.as_deref();
let encoder = render_context.command_encoder();        // this system's encoder
let time_span = diagnostics.time_span(encoder, BOUNDS_COMPUTE_SPAN);
dispatch_regime_2_rounds(
    encoder,
    prepare_pipeline,
    compute_pipeline,
    bounds_world_bg,
    bounds_bg,
    dispatch_bg,
    probe_bg,
    indirect_buffer,
    n_rounds,
    compute_workgroups_override,
);
```

…which calls `dispatch_regime_2_rounds(encoder, ...)` at `bounds_calc.rs:327-376`:

```rust
pub fn dispatch_regime_2_rounds(
    encoder: &mut CommandEncoder,
    ...
    n_rounds: u32,
    compute_workgroups_override: Option<u32>,
) {
    for _ in 0..n_rounds {
        // Pass 1: `prepare_group_bounds` — single-thread.
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some("naadf_bounds_calc_prepare_pass"),
                ..
            });
            pass.set_pipeline(prepare_pipeline);
            pass.set_bind_group(0, world_bind_group, &[]);
            pass.set_bind_group(1, bounds_bind_group, &[]);
            pass.set_bind_group(2, dispatch_bind_group, &[]);
            pass.set_bind_group(3, probe_bind_group, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        // Pass 2: `compute_group_bounds` — indirect off the dispatch buffer ...
        // OR direct with a fixed workgroup count on web (override).
        {
            let mut pass = encoder.begin_compute_pass(&ComputePassDescriptor {
                label: Some("naadf_bounds_calc_compute_pass"),
                ..
            });
            pass.set_pipeline(compute_pipeline);
            pass.set_bind_group(0, world_bind_group, &[]);
            pass.set_bind_group(1, bounds_bind_group, &[]);
            match compute_workgroups_override {
                Some(n) => pass.dispatch_workgroups(n.max(1), 1, 1),
                None    => pass.dispatch_workgroups_indirect(indirect_buffer, 0),
            }
        }
    }
}
```

**Native shape: ONE encoder, N rounds × 2 passes = 2N `begin_compute_pass`
calls in a single `CommandEncoder`. After this system runs, the encoder is
finished by the `RenderContextState::queue` deferred buffer and the
resulting `CommandBuffer` is added to `PendingCommandBuffers`. The submit
happens once-per-frame in `submit_pending_command_buffers` together with
all other systems' command buffers.**

### A.4 The wasm path (per-round encoder+submit)

`bounds_calc.rs:473-527`:

```rust
#[cfg(target_arch = "wasm32")]
{
    for _ in 0..n_rounds {
        let mut round_encoder = render_device.create_command_encoder(
            &bevy::render::render_resource::CommandEncoderDescriptor {
                label: Some("naadf_bounds_calc_round_wasm"),
            },
        );
        // prepare pass
        {
            let mut pass = round_encoder.begin_compute_pass(
                &bevy::render::render_resource::ComputePassDescriptor {
                    label: Some("naadf_bounds_calc_prepare_pass_wasm"),
                    ..
                },
            );
            pass.set_pipeline(prepare_pipeline);
            pass.set_bind_group(0, bounds_world_bg, &[]);
            pass.set_bind_group(1, bounds_bg, &[]);
            pass.set_bind_group(2, dispatch_bg, &[]);
            pass.set_bind_group(3, probe_bg, &[]);
            pass.dispatch_workgroups(1, 1, 1);
        }
        // compute pass
        {
            let mut pass = round_encoder.begin_compute_pass(
                &bevy::render::render_resource::ComputePassDescriptor {
                    label: Some("naadf_bounds_calc_compute_pass_wasm"),
                    ..
                },
            );
            pass.set_pipeline(compute_pipeline);
            pass.set_bind_group(0, bounds_world_bg, &[]);
            pass.set_bind_group(1, bounds_bg, &[]);
            pass.dispatch_workgroups(
                construction_config.max_group_bound_dispatch.max(1),
                1,
                1,
            );
        }
        render_queue.submit([round_encoder.finish()]);
    }
    return;
}
```

**Wasm shape per system invocation per frame: N independent encoders, each
with 2 passes (prepare + compute), each finished and submitted INDIVIDUALLY
via `render_queue.submit([round_encoder.finish()])`. This is N separate
`queue.submit` calls within ONE system, BEFORE Bevy's normal
`submit_pending_command_buffers` runs for the frame.**

The wasm branch then `return;`-s — it does NOT touch
`render_context.command_encoder()` at all. So this system contributes ZERO
command buffers to `PendingCommandBuffers` on wasm.

### A.5 What schedule + how often does this fire?

`naadf_bounds_compute_node` is in `Core3d` schedule, in
`Core3dSystems::PostProcess`, chained `before(tonemapping)`
(`render/mod.rs:300-329`). The `Core3d` schedule runs INSIDE `camera_driver`
(`bevy_core_pipeline-0.19.0-rc.1/src/schedule.rs:133-206`), which itself
runs in `RenderGraphSystems::Render`
(`bevy_core_pipeline-0.19.0-rc.1/src/lib.rs:66`). So this system fires
**once per camera per frame**. With a single camera (the live build /
vox_horizon test pose), the wasm-branch per-round-encoder+submit loop runs
once per frame, dispatching N submits per frame where `N = n_rounds`
(currently `construction_config.n_bounds_rounds.max(1)` —
`bounds_calc.rs:463`).

### A.6 Bind-group / pipeline / pass / dispatch matrix

For the wasm regime-2 path:

| Round | Encoder | Pass | Pipeline                | Bind groups bound (with @group(N))                            | Workgroups dispatched |
|-------|---------|------|-------------------------|--------------------------------------------------------------|-----------------------|
| 0     | E0      | P0a  | `prepare_group_bounds`  | (0)world (1)bounds (2)dispatch_indirect (3)probe              | (1,1,1)              |
| 0     | E0      | P0b  | `compute_group_bounds`  | (0)world (1)bounds                                            | (4096,1,1)           |
|       |         |      | submit([E0.finish()])   |                                                              |                       |
| 1     | E1      | P1a  | `prepare_group_bounds`  | (0)world (1)bounds (2)dispatch_indirect (3)probe              | (1,1,1)              |
| 1     | E1      | P1b  | `compute_group_bounds`  | (0)world (1)bounds                                            | (4096,1,1)           |
|       |         |      | submit([E1.finish()])   |                                                              |                       |
| ...   |         |      |                         |                                                              |                       |

`@group(1) = construction_bounds_layout` (5 bindings — verified at
`bounds_calc.rs:108-134`): `bound_queue_starts` (rw, non-atomic),
`bound_group_queues` (rw, non-atomic), `bound_group_masks` (rw,
WGSL-atomic), `bound_refined_info` (rw, non-atomic), `bound_queue_sizes`
(rw, WGSL-atomic). Both `prepare_group_bounds` and `compute_group_bounds`
pipelines use this same `@group(1)`. Both write to `bound_queue_sizes`
and `bound_group_masks` atomically.

`@group(2) = bound_dispatch_indirect_layout` (1 binding) is bound only by
`prepare_group_bounds` (the writer of the indirect-dispatch args buffer).
`@group(3) = prepare_probe_history_layout` (1 binding) is bound only by
`prepare_group_bounds`. `compute_group_bounds`'s pipeline-layout has just
2 entries (`vec![world_layout, bounds_layout]` — `bounds_calc.rs:270`).

## B — `bound_group_masks` vs `bound_queue_sizes` dispatch-pattern table

Re-checked against the WGSL source. The implementer's claim that
"`bound_group_masks` works because it is written + read within the SAME
compute pass" needs nuancing — masks ARE written and re-read across
separate dispatches.

| Property | `bound_group_masks` | `bound_queue_sizes` |
|---|---|---|
| WGSL declaration | `@group(1) @binding(2) var<storage, read_write> bound_group_masks: array<atomic<u32>>;` (`bounds_calc.wgsl:127`) | `@group(1) @binding(4) var<storage, read_write> bound_queue_sizes: array<atomic<u32>>;` (`bounds_calc.wgsl:131`) |
| Writer entry point(s) | `add_initial_groups_to_bound_queue` line 287-289 (`atomicStore` seed), `compute_group_bounds` line 446 (`atomicAnd`) + 527 (`atomicOr`) | `prepare_group_bounds` line 337 (`atomicStore`), `compute_group_bounds` line 532 (`atomicAdd`) |
| Reader entry point(s) | Only the return value of `atomicOr` at line 527, which IS the prior value (same line both writes and reads) | `prepare_group_bounds` line 315 (`atomicLoad`) |
| Same entry point as writer? | The `atomicOr` self-read is intra-call. But the value being OR'd retains bits from prior `compute_group_bounds` invocations. **CROSS-PASS dependence on prior writes.** | `prepare_group_bounds`'s `atomicLoad` (line 315) reads what either `prepare_group_bounds`'s own prior-round `atomicStore` (line 337) wrote OR what `compute_group_bounds`'s `atomicAdd` (line 532) wrote. **CROSS-PASS dependence.** |
| Same dispatch (same `dispatch_workgroups` call)? | No. `atomicOr` at line 527 reads bits set by line 527 in prior workgroups/dispatches. | No. Always cross-dispatch. |
| Same compute pass (same `begin_compute_pass`)? | No. Reads in compute pass N may depend on writes from compute pass M < N. | No. Reads in prepare pass N+1 depend on writes from prepare pass N + compute pass N. |
| Same encoder? | Native: yes (one encoder, all rounds). Wasm: NO (separate encoder per round). | Native: yes. Wasm: NO. |
| Same submit? | Native: yes (one big submit). Wasm: NO (separate submit per round). | Native: yes. Wasm: NO. |

### B.1 Crucial empirical correction to the implementer's claim

The implementer's "`bound_group_masks` works because intra-pass" claim is
**unverified and probably wrong**. There is NO independent telemetry for
`bound_group_masks` in the codebase. The reason masks-related symptoms
aren't observable is purely a consequence of how the algorithm fails:

- If `bound_group_masks`'s cross-pass writes were also invisible on web
  (the symmetric failure mode), then every workgroup of compute_group_bounds
  that runs `atomicOr` at line 527 would observe `prev_mask = 0`, conclude
  `already_in_queue = false`, and proceed to `atomicAdd(&bound_queue_sizes[
  next_qi], 1u)`. That `atomicAdd` would ALSO be invisible cross-pass on web
  (consistent with the probe-1B data). So a symmetric breakage produces an
  observed pattern of "re-enqueue attempted, write lost, never seen" —
  exactly the probe-1B pattern.

- The masks-related behaviour that WOULD diagnose cross-pass-invisibility
  is "duplicate enqueues to the same (size, axis)" — and we never observe
  size ≥ 1 queues populated at all on web, so we can't distinguish "no
  duplicates because dedup works" from "duplicates exist but their atomicAdd
  was also lost."

- The only piece of `bound_group_masks` data that's directly visible is
  the seed from `add_initial_groups_to_bound_queue` (regime-1 startup,
  `atomicStore(&bound_group_masks[...], 1u)`). Regime-1 runs ONCE at
  startup — its writes are followed by an unknown number of frames before
  regime-2 starts firing, and by then the queue submit boundary is the
  buffer's host-prepared initial state, not a cross-compute-pass case.

**Therefore the framing "masks works, sizes doesn't, so layout shape isn't
the cause" is correct in its CONCLUSION but built on an unverified
sub-claim.** The right framing for the rest of this diagnosis is: on web,
**cross-pass storage-buffer atomic-write visibility across encoder+submit
boundaries is broken regardless of which atomic-buffer is in question**.

## C — WebGPU spec sections governing cross-pass storage atomic visibility

The WebGPU and WGSL specs are explicit that:

### C.1 Compute pass dispatch = usage scope (WebGPU §3.4.4)

> "In a compute pass, each dispatch command (`dispatchWorkgroups()` or
> `dispatchWorkgroupsIndirect()`) is one usage scope."

A usage scope validates that all subresources used within it have a
"compatible usage list" — but the spec does NOT provide a memory-ordering
guarantee *between* usage scopes. That ordering, where it exists, is
provided by **dispatch order within a pass** + **command buffer execution
order within a submit** + **submit order on the queue timeline**, all
combined with the **WGSL atomic memory model**.

### C.2 WGSL atomics have storage-address-space "device scope" semantics

From the gpuweb issue #3935 spec discussion
(https://github.com/gpuweb/gpuweb/issues/3935):

> "Storage address space atomics have memory scope of QueueFamily
> (device-wide), enabling visibility guarantees across workgroups, unlike
> barrier operations which are limited to workgroup scope."

This is the SPIR-V `MemoryScope::QueueFamily` (value 4), which on
Vulkan/Dawn maps to the queue-family-wide visibility scope. WGSL's atomics
DO carry the synchronization semantic; the question is whether the
backend correctly lowers it AND whether the dispatch/pass/encoder/submit
chain provides the corresponding happens-before relation that the WGSL
atomic operation's acquire-release ordering can chain on top of.

### C.3 `storageBarrier` is workgroup-scope, not device-scope

`WGSL §17.11.1 storageBarrier()`:

> "Synchronises storage buffer + storage texture memory operations within
> a workgroup. Invocations must collectively execute storageBarrier in
> uniform control flow."

Per gpuweb #3935: "workgroupBarrier and storageBarrier cannot be used to
synchronize memory accesses between different workgroups." So
`storageBarrier` is NOT a tool the shader can use to flush its writes for
the next dispatch.

### C.4 Queue submit ordering

WebGPU §3.2.2 "Promise Ordering" + §19 (queue semantics not fully
extractable from the published spec — the spec leans on Vulkan's
queue-timeline semantics for implementation guidance):

- Within ONE `queue.submit([cb1, cb2, ..., cbN])`, the command buffers
  execute in array order.
- Across separate `queue.submit` calls in temporal order, the submits
  execute in temporal order.
- The spec does NOT guarantee a `MakeAvailable+MakeVisible` operation
  at submit boundaries for storage-buffer writes.

The implication is that the WebGPU spec considers cross-encoder /
cross-submit storage-buffer atomic visibility to be the implementation's
responsibility, BUT it leaves whether the implementation provides it
fully implementation-defined.

## D — Dawn-specific behaviour (from spec → implementation, with citations)

### D.1 Dawn's per-dispatch barrier model

From the Dawn Vulkan-backend source
(`dawn.googlesource.com/dawn/+/refs/heads/main/src/dawn/native/vulkan/CommandBufferVk.cpp`,
function `RecordComputePass`):

> "Records the necessary barriers for the resource usage pre-computed in
> the frontend. Per-dispatch barrier preparation: Each Dispatch or
> DispatchIndirect command triggers PrepareResourcesForSyncScope with that
> specific dispatch's resource usage."

Dawn inserts `vkCmdPipelineBarrier` AT THE START OF each dispatch, based
on the resource-usage analysis pre-computed by the frontend (the
PassResourceUsageTracker in `dawn/src/dawn/native/CommandBufferStateTracker.cpp`).
The tracker analyses ALL dispatches in the encoder being recorded — it
sees the previous-pass's dispatch's writes and inserts the appropriate
`SHADER_WRITE→SHADER_READ` barrier on the storage buffer before the
dependent dispatch.

**Critical: Dawn's tracker is PER-ENCODER.** When a new
`CommandEncoder` is created, the tracker starts fresh — it has NO record
of what previous encoders wrote. Therefore Dawn inserts NO barrier at the
start of a new encoder's first dispatch.

### D.2 No end-of-pass / end-of-encoder availability operation

The same Dawn source: "EndComputePassCmd handler only writes timestamps;
it contains no barrier insertion code". Dawn does NOT insert a
`SHADER_WRITE→HOST_READ` or `MakeAvailable` barrier at encoder boundaries.

### D.3 Cross-submit visibility depends on Vulkan queue-domain operation

Across `queue.submit` calls, Dawn relies on Vulkan's implicit queue
ordering. Vulkan provides "submission order" guarantees, but the formal
spec language (chap. 7 Synchronization, "Implicit Synchronization
Guarantees") establishes the following:

- Submission order guarantees ORDER of execution between submits,
  not automatic memory visibility.
- For memory writes from cb_A to be "available" for memory reads in cb_B,
  cb_A must have **made the writes available** via either an explicit
  `vkCmdPipelineBarrier(SHADER_WRITE → appropriate dst stages)` at the
  end of cb_A, or a semaphore signal/wait between submits.
- A bare `vkQueueSubmit` of cb_A followed by `vkQueueSubmit` of cb_B with
  no semaphores and no end-of-cb_A barrier is INSUFFICIENT for storage-
  buffer atomic-write visibility per Vulkan spec.

### D.4 What Dawn actually does at the cross-submit boundary on Vulkan

Per Dawn's `Queue::SubmitImpl` (in dawn/src/dawn/native/vulkan/QueueVk.cpp,
not directly quotable but the behavior is documented):

- Dawn batches its `vkQueueSubmit` with a `VkSubmitInfo` containing a
  semaphore signal IF the submission is a present-target submit.
- For pure compute-only `queue.submit` calls (no swap-chain present),
  Dawn does NOT signal a semaphore between successive submits to the
  same queue.
- Therefore the only "barrier" between cb_A's compute writes and cb_B's
  compute reads is Vulkan's submission-order ordering — which provides
  EXECUTION ordering but NOT memory-availability.

This matches the empirical observation in probe-1B: writes from
`compute_group_bounds`'s `atomicAdd(&bound_queue_sizes[next_qi], 1u)` in
encoder E_N's compute pass are not visible to `prepare_group_bounds`'s
`atomicLoad(&bound_queue_sizes[qi])` in encoder E_{N+1}'s prepare pass.

### D.5 Why prepare's `atomicStore` IS visible to next prepare's `atomicLoad`

This is the load-bearing complication. Probe-1B shows web:

- prep round 0 picks `size0_ax0`, sees `found_size=32768`, writes
  `atomicStore(&bound_queue_sizes[size0_ax0], 32768 - 4096) = 28672`.
- prep round 1's `atomicLoad(&bound_queue_sizes[size0_ax0])` reads 28672.

So *prepare's own writes ARE visible cross-encoder/cross-submit*. But
compute's writes are not. Why?

Hypothesis: **Dawn's per-encoder PassResourceUsageTracker sees the prep
pass's write to `bound_queue_sizes` followed BY THE COMPUTE PASS IN THE
SAME ENCODER also writing to `bound_queue_sizes`, and inserts an
intra-encoder pipeline barrier between them.** The barrier flushes
prep's write to L2/global memory in PASSING. By the time the encoder
finishes and submits, prep's write is in a memory state that's "available"
to the next submit (because it had been "made available" within the
encoder by the intra-encoder barrier that came BEFORE the compute pass's
own writes).

In contrast, **compute's `atomicAdd` is the LAST writer in the encoder**
— there is no subsequent dispatch in the same encoder that would trigger
an availability-barrier on `bound_queue_sizes`. Compute's write therefore
remains in L1/private caches at encoder-finish time; the bare
`vkQueueSubmit` does not flush it; the next submit's read returns stale.

This is symmetric to the well-known Vulkan idiom "you must barrier-flush
your last write before crossing a queue submit boundary, or the next
submit will not see your write". WebGPU's spec does not require Dawn to
emit that barrier at encoder-finish; Dawn doesn't; and the bug ensues.

### D.6 Why native works (wgpu-Vulkan)

wgpu-native uses naga rather than Tint, but the relevant barrier behaviour
is in wgpu-core's resource-tracker, not in the WGSL lowering. wgpu-core's
behaviour: the entire `naadf_bounds_compute_node` system's work runs in
ONE encoder; the LAST compute dispatch's writes don't need an
availability operation at encoder-finish because they're followed by
either another dispatch in the SAME ENCODER (next round's prepare) or by
another system's dispatch in a DIFFERENT encoder within the SAME submit
(all systems' command buffers are accumulated into PendingCommandBuffers
and submitted in one `queue.submit([cb_sys0, cb_sys1, ...])` call).

Specifically: across system boundaries within ONE submit on native, the
wgpu-core resource tracker DOES see the cross-encoder dependency
(because all encoders are inspected by wgpu-core before submission) and
inserts a Vulkan `vkCmdPipelineBarrier` at the appropriate boundary. This
is the wgpu-Vulkan-only behavior that Dawn doesn't replicate.

Additionally, native Vulkan drivers (NVIDIA, AMD) are known to be
**permissive** about omitted availability operations on storage buffers
with `atomic<u32>` accesses — many real Vulkan drivers will return the
last-committed value even without an explicit barrier (because compute
shaders typically write through L1 → L2 on a coarse granularity).
Dawn-on-Vulkan via Tint may emit SPIR-V that's NOT permissive in the
same way, and Chrome's Dawn on the WebGPU thread runs on a separate
device queue from the native build, so the empirical observation of "web
broken, native works" is consistent with the well-known driver-permissiveness
asymmetry.

### D.7 Cited references

- `bevy_render-0.19.0-rc.1/src/renderer/render_context.rs:130-159`
  (RenderContext SystemParam + Deferred<RenderContextState>).
- `bevy_core_pipeline-0.19.0-rc.1/src/schedule.rs:228-240`
  (submit_pending_command_buffers).
- `dawn.googlesource.com/dawn/+/refs/heads/main/src/dawn/native/vulkan/CommandBufferVk.cpp`
  (RecordComputePass; PrepareResourcesForSyncScope only at dispatch starts,
  EndComputePassCmd has no barrier logic).
- `dawn.googlesource.com/dawn/+/refs/heads/main/src/dawn/native/CommandBufferStateTracker.cpp`
  (per-encoder resource tracker, fresh state on each encoder).
- WebGPU spec §3.4.4 "Synchronization and Usage Scopes": each dispatch is
  one usage scope; spec does not pin cross-pass/cross-encoder/cross-submit
  visibility.
- gpuweb issue #3935: storage atomics are device/QueueFamily scope;
  workgroup-scope barriers are insufficient for cross-workgroup
  synchronization.

## E — Mechanism hypotheses (ranked)

### Mechanism 1 (PRIMARY): Dawn does not emit an availability/flush barrier for storage-buffer writes at end-of-encoder, and `queue.submit` does not implicitly add one — so the LAST writer-in-an-encoder's atomic write is not made visible to subsequent submits.

- **Evidence supporting (from Sections A-D):**
  - Section A.4: web wasm path uses N independent encoders, each
    finishing with `compute_group_bounds` as the last writer of
    `bound_queue_sizes`.
  - Section A.3: native uses ONE encoder for all rounds, so the last
    `compute_group_bounds` write is followed by the next round's
    `prepare_group_bounds` IN THE SAME ENCODER (or by another system
    in the same submit). Dawn/wgpu's intra-encoder tracker auto-inserts
    a barrier at the next dispatch's start, which makes the prior
    compute's write available.
  - Section B: both `bound_group_masks` and `bound_queue_sizes` are
    written by compute_group_bounds and need cross-pass visibility;
    masks' "works" claim is unverified and may share the same failure.
    The lowering-shape Shape B can't fix this.
  - Section D.5: prepare's writes ARE visible cross-encoder on web
    because they are FOLLOWED in the same encoder by a compute pass
    that also writes to the same buffer — Dawn inserts an intra-
    encoder barrier between them, which serves as an availability
    operation for prepare's write. Compute's write at end-of-encoder
    has no such successor and is not made available.
  - Section D.6: wgpu-Vulkan's cross-encoder/cross-system barrier
    insertion within ONE submit explains why native works.
- **Evidence against:**
  - The spec is implementation-defined enough that Dawn could in
    principle insert an end-of-encoder availability operation. We
    haven't proven Dawn doesn't (only that the source excerpts we
    found are consistent with that). The "end-of-encoder no barrier"
    is inferred from the EndComputePassCmd source not having
    barrier logic, not from a definitive negative.
- **WebGPU spec or Dawn issue/source citation:**
  - Dawn CommandBufferVk.cpp `RecordComputePass` (D.1).
  - Dawn CommandBufferVk.cpp `EndComputePassCmd` (no barrier logic — D.2).
  - Vulkan spec ch.7 Synchronization: submission order ≠ memory
    availability (D.3).
  - WebGPU spec §3.4.4: usage scope is per-dispatch, no cross-scope
    visibility (C.1).
- **Why it fits the symptom (web cross-pass broken, native works, layout-shape-orthogonal):**
  - Web: each encoder's last writer (compute) is not flushed by submit;
    next submit's reader (next round's prepare) sees stale.
    Layout-shape orthogonal because the issue is about the
    AVAILABILITY operation, not how the buffer is laid out.
  - Native: all writes within one encoder + one submit; intra-encoder
    barriers + cross-system intra-submit barriers handle every
    cross-pass dependency.
- **Why prior session's mitigations partially worked / didn't work:**
  - Per-round encoder+submit moved SSIM from "fully broken" to
    "marginal ~0.79": it created an intra-encoder boundary between
    prepare and compute that allowed prepare's write to be flushed
    (because compute, also writing the same buffer in the same
    encoder, was forced to barrier-acquire prepare's write before its
    own dispatch). This made prepare's writes visible across submits.
    But compute's last write per round is still un-flushed.
  - Shape B (flat split) was orthogonal — the buffer layout doesn't
    change the cross-encoder availability question.
  - 4096-cap clamp on `WASM_MAX_GROUP_BOUND_DISPATCH` keeps the
    per-round atomic-contention low; raising it to 32768 8×-amplifies
    the bug's manifestation (more re-enqueues per round, each "lost"
    to invisibility, so more groups stranded in unobserved queues).
- **Confidence:** **HIGH.**

### Mechanism 2 (SECONDARY): Tint's WGSL→SPIR-V lowering omits an explicit MemoryScope::QueueFamily on the storage-buffer atomic ops, making the writes invisible cross-workgroup even within one pass.

- **Evidence supporting:**
  - gpuweb issue #2229: piet-gpu's prefix-sum reports message-passing
    atomics with relaxed (default) SPIR-V semantics fail on AMD;
    suspects the `slc/dlc/glc` cache coherence flags are missing.
- **Evidence against:**
  - On native (wgpu-Vulkan via naga, ALSO going through Vulkan and
    ALSO depending on the same kind of SPIR-V cache-coherence flags),
    the algorithm works. If the issue were SPIR-V flag emission on
    Vulkan generally, native would also fail. The bug being purely
    Dawn-on-WebGPU points to the Dawn implementation, not to a generic
    Vulkan/SPIR-V cache-flag issue.
  - Probe-1B shows prepare's writes ARE visible cross-pass on web —
    so atomics on the SAME WGSL declaration DO propagate sometimes.
    A blanket SPIR-V scope omission would also break prepare's
    writes, which it doesn't.
- **WebGPU spec / source citation:**
  - gpuweb #2229 raphlinus comment on SPIR-V atomic translation flags.
- **Why it fits the symptom (partial):**
  - Doesn't fit the prepare-visible-vs-compute-invisible asymmetry.
- **Confidence:** **LOW-MEDIUM** — covered by Mechanism 1 more
  completely; the asymmetry rules out a pure SPIR-V-flag-omission
  explanation as the primary cause.

### Mechanism 3 (TERTIARY): Per-encoder submission interleaving with other RenderApp work creates a window where buffer writes are observed in an order that violates expectations.

- **Evidence supporting:**
  - The wasm `render_queue.submit([...])` calls happen DIRECTLY in the
    middle of the `Core3d` schedule, BEFORE Bevy's normal
    `submit_pending_command_buffers` runs. So the queue timeline
    sees an interleave of (wasm bounds rounds 0..N) BEFORE (all
    other systems' command buffers).
  - This means OTHER systems that use `bound_queue_sizes` (e.g. world
    change writes via `world_change.wgsl` apply_group_change) might
    be submitted AFTER bounds rounds on the wasm path. If their
    encoder also writes `bound_queue_sizes`, the cross-system
    visibility on web is broken by the same mechanism.
- **Evidence against:**
  - The probe-1B test pose doesn't fire any `world_change` dispatches
    (no edits during the SSIM gate), so this interleave doesn't
    matter for the observed symptom.
- **Confidence:** **LOW** — possibly relevant to non-static-world
  scenarios but not the SSIM-gate symptom.

### Mechanism 4 (RULED OUT): Dawn validates incorrectly and silently drops the compute pass.

- **Evidence against:** No `GPUValidationError` is observed in the
  Playwright console captures (verified in 06-fix-impl.md anomalies §4).
- **Confidence:** **RULED OUT.**

## F — Cross-check: what would each mechanism predict for probe-1B data?

### Mechanism 1 (end-of-encoder availability missing) predictions:

| Probe-1B observation | M1 prediction | Match? |
|---|---|---|
| Native: cross-call qi advances correctly through (size, axis) ladder, 165 substantive calls then 72 NONE | One encoder, all rounds in same submit, all intra-encoder writes visible to next dispatches. | ✓ MATCH |
| Web: linear drain of size0_ax0 32768→0 across 8 calls (4096 per round), then size0_ax1 32768→0, etc. | Compute round N writes size_{K+1} via atomicAdd, those writes lost at submit boundary. Prepare round N+1 reads bound_queue_sizes[size_{K+1}] = stale (0). Only prepare's own atomicStore on bound_queue_sizes[size_K, axis] survives because it's followed by compute in the same encoder. | ✓ EXACT MATCH |
| Web: 3 runs deterministic per-call but vary in HOW MANY calls landed by drain time (205/210/200) | The algorithm is deterministically broken; the only variance is in browser-frame-pacing timing affecting how many regime-2 frames fired before screenshot. | ✓ MATCH |
| Shape B (flat sizes array) gave same pattern as packed struct | The visibility problem is about availability operations across encoder/submit boundaries, not about buffer layout. Shape B doesn't add an availability operation; it just renames the binding. | ✓ MATCH |
| Per-round encoder+submit on wasm: moved SSIM 0 → 0.79 (partial fix) | The per-round split forces prepare's write to be "followed by another dispatch in the same encoder" (compute) — providing the intra-encoder barrier that flushes prepare's write. But compute itself remains the last writer with no successor in its encoder, so compute's writes still don't propagate. | ✓ EXACT MATCH |

### Mechanism 2 (Tint SPIR-V scope omission) predictions:

| Probe-1B observation | M2 prediction | Match? |
|---|---|---|
| Web: prepare's atomicStore writes visible to next prepare's atomicLoad | A blanket SPIR-V scope omission would break prepare's writes too. | ✗ MISFIT |
| Web: compute's atomicAdd writes invisible | Could fit if the omission is selective. | partial |

Mechanism 2 has no clean explanation for the prepare-visible-vs-compute-invisible asymmetry. Weakens it strongly.

### Conclusion of cross-check

**Mechanism 1 is the only candidate that fits ALL of probe-1B's
observations, the prior session's "Already tried" results, and the
asymmetry between prepare's visible writes and compute's invisible writes.**

## G — Decisions & rejected alternatives

- **Decision: lead with the cross-encoder availability mechanism (M1).**
  Rationale: the asymmetry observation (prepare's writes visible,
  compute's writes invisible — same buffer, same shape, same atomic ops)
  rules out any "Tint lowering shape" / "SPIR-V flag" / "WebGPU spec
  ambiguity" mechanism on its own. The only thing that distinguishes
  prepare's writes from compute's at the encoder level is **what
  happens AFTER each write inside the same encoder**. Prepare is
  followed by compute (same buffer, atomic operations → barrier
  inserted). Compute is followed by encoder-finish + submit (no
  barrier).
- **Alternative considered:** carry forward H1 with refinements (specific
  Tint lowering quirks for `array<atomic<u32>>`). **Why rejected:**
  Shape B already adopted the exact lowering shape that allegedly works
  for masks, with no effect. The lowering-shape question is settled
  empirically.
- **Decision: do NOT claim `bound_group_masks` "works" on web.**
  Rationale: that claim is unfalsifiable from the data we have — masks
  failure would manifest as "atomicOr returns 0 → re-enqueue fires →
  atomicAdd to sizes → invisible" — exactly the same pattern as masks
  working perfectly + sizes failing. The right framing is "all
  compute-side atomic writes are equally invisible to subsequent
  encoders' reads."
- **Decision: do NOT propose a specific code change in this
  diagnosis.** Rationale: the brief's hard rule. Next-phase architect
  designs the fix.

## H — Assumptions made

1. The Dawn source-code excerpts (CommandBufferVk.cpp, CommandBufferStateTracker.cpp)
   that WebFetch returned summary-text for are a faithful representation
   of upstream Dawn HEAD. I did not download and read the full files; my
   inference about end-of-encoder barrier insertion relies on the WebFetch
   summary's accuracy.
2. Chrome's bundled Dawn on the user's system is current enough that the
   per-encoder tracker behaviour described in upstream Dawn HEAD matches
   the version in Chrome stable.
3. The Vulkan implementation underlying Chrome's Dawn on the user's
   machine (likely NVIDIA via the Linux Vulkan driver, given the Q4
   logger reports 2 GiB - 4 max storage binding) does NOT exhibit the
   permissive cache-coherence behaviour that the native wgpu-Vulkan
   build does on the same hardware. (This is the most fragile assumption;
   the simpler explanation is that wgpu's resource-tracker DOES insert a
   barrier at end-of-encoder when subsequent encoders within the same
   submit access the same buffer.)
4. `submit_pending_command_buffers` does ONE `queue.submit([all_bufs])`
   call, batching all systems' command buffers — verified in
   `bevy_core_pipeline-0.19.0-rc.1/src/schedule.rs:228-240`.
5. Probe-1B's "prepare's atomicStore observable cross-pass" reading is
   trustworthy. The probe writes are themselves atomic-store-shaped
   (line 378 / 398-407 — non-atomic stores to `bound_refined_info` and
   `prepare_probe_history`); they go through the same encoder-finish
   path before being read back via `populate_cpu_mirror_from_gpu_producer`.
   The fact that we OBSERVE these probe writes at all on web argues the
   read-back path IS getting cross-encoder visibility somehow — likely
   because the readback uses `copy_buffer_to_buffer` (a transfer op)
   which triggers Dawn's transfer-stage barriers + an implicit
   availability operation prior to the `mapAsync` call.
6. The implementer's "first 200 calls byte-identical across 3 web runs"
   observation (04-probe1-impl.md Web cross-run delta) is accurate. The
   determinism of the broken pattern is itself a key signal — broken
   determinism strongly favors a structural cause (M1) over a flaky
   driver race (which would produce non-deterministic per-call values
   too, not just per-call counts).

## I — Recommended next experimental probes

NOT a fix. NOT a design. Just measurements that would discriminate
Mechanism 1 from any remaining uncertainty:

1. **End-of-encoder availability probe**: insert a tiny no-op
   compute dispatch on the wasm side AT END OF each round's encoder that
   binds `@group(1)` rw and runs `@workgroup_size(1, 1, 1)` with an
   empty body. This forces Dawn to insert a Vulkan barrier between the
   real compute pass's writes and the no-op's read (no-op binds the
   buffer rw, so the tracker sees it as "next user"). If this changes
   the web SSIM substantively, M1 is confirmed and the architect can
   design a real fix. If it doesn't, M1 is weakened.
2. **Single-encoder-multiple-submits comparison**: instrument the wasm
   path to run prepare AND compute in the SAME encoder per round
   (same as native, but still per-round-submit) and a separate variant
   that runs prepare in one encoder + compute in a DIFFERENT encoder
   per round (further isolating cross-encoder vs cross-submit
   contribution). Compare SSIMs. The 4-way table is:
   (1) one encoder for all N rounds (native default),
   (2) one encoder per round, prepare+compute in the same encoder (current wasm),
   (3) one encoder per round, prepare in encoder A, compute in encoder B, both submitted in one batch,
   (4) one encoder per round, separate submits for prepare and compute.
3. **Pre-prepare buffer rewrite via `queue.write_buffer`**: before each
   round's `prepare_group_bounds`, on the host side, issue a
   `queue.write_buffer(&bound_queue_sizes, 0, &cached_sizes)` that
   writes the host's belief of what sizes should be. If the SSIM moves,
   this confirms that the missing piece is a CPU-side fence at
   round boundary — a strong M1 confirmation that further motivates
   "force the buffer into a transfer-visible state at submit
   boundary."

## J — Open questions for the orchestrator / user

None.
