# Implementors' shared notebook — wasm-chunk-aadf-determinism

Shared private channel between consecutive implementor-agent dispatches.
The orchestrator does NOT read this unless the probe is won. Append per
attempt. Be dense and code-grounded; future-you reads this cold.

---

## Dispatch 1 — first implementor

### Context loaded
- Worktree clean on entry (probe-2 noop + Shape-B split still applied).
- All round-1 and round-2 diagnostic docs read in full.
- Read `bounds_calc.rs` (696 lines), `bounds_calc.wgsl` (579 lines),
  `mod.rs` selected regions, `config.rs` (full).

### Anchoring observations re-confirmed by reading source

1. **Algorithm shape, wasm path** (`bounds_calc.rs:528-610`):
   For each of `n_rounds` (5) per Core3d-system invocation:
     - new encoder
     - prepare pass (1,1,1)  — reads sizes[qi], writes refined_info, dispatch_indirect
     - compute pass (max_group_bound_dispatch=4096, 1, 1)  — writes chunks, masks, queues, sizes (via atomicAdd)
     - end-of-encoder-noop pass (1,1,1)  — atomicLoad+atomicStore on sizes[0]  (probe-2; refuted)
     - `render_queue.submit([encoder.finish()])`

2. **Native path** (`bounds_calc.rs:649-664`):
   ONE encoder for all 5 rounds, finished by Bevy's normal `submit_pending_command_buffers`.
   So all 5×{prepare, compute} pairs sit in one command buffer that's submitted alongside all other systems' command buffers in a single `queue.submit([cb_sys0, cb_sys1, ..., cb_sysN])`.

3. **Probe-1B telemetry pattern** (from `04-probe1-impl.md`):
   - Native: 165 prepare calls, deterministic, walks every (size 0..30 × axis 0..2) with found_size=32768 once each, then 72 NONE calls. Native compute's atomicAdd to next-size queues IS visible to next round's prepare.
   - Web: 200-215 prepare calls, deterministic per-call values across runs, BUT pattern is "drain size0_ax0 linearly 32768→28672→24576→…→4096→0 (8 calls), then drain size0_ax1 the same way, then size0_ax2, ... reaching size8 by call ~200". Web compute's atomicAdd to OTHER-slot queues is NOT visible.

4. **Asymmetry**: Same-slot atomicStore/atomicLoad (prepare's own subsequent reads) IS visible. Different-slot atomicAdd from compute is INVISIBLE across submits.

5. **Compute IS reaching size 8** on web — so SOMETHING propagates the values, just very slowly. Possibly batched flushes every N submits.

### Strategy posture

The two prior implementor approaches addressed putative root causes:
- Shape B (split into flat array<atomic<u32>>): targeted "wrong lowering shape" → falsified.
- Probe-2 noop (end-of-encoder shader read+write to force Dawn barrier): targeted "Dawn doesn't insert end-of-encoder availability barrier" → falsified.

What's NOT been tried in this worktree's iteration history (per the brief):
- Reverting to ONE-encoder-per-system on wasm (same as native).
- `copy_buffer_to_buffer` between rounds (Shape F).
- Reversing dispatch order so prepare follows compute.
- CPU-side queue.write_buffer fence between rounds (probe 3 from round-2).
- Algorithmic restructure: rebuild queue state from masks via scan kernel (Shape G).
- Doing the regime-2 work as part of Bevy's normal encoder (deferring to `submit_pending_command_buffers`) rather than per-round-submit.

### Hypotheses ranked (with reasoning)

**H1: The per-round-encoder+submit pattern is itself harmful.** Native uses one encoder/submit per frame and works. The wasm path uses N submits per frame. The handoff says "moved SSIM from broken→0.79" but that data is from a prior session that had a DIFFERENT prior baseline (with broken indirect dispatch + larger workgroup count). In current tree, perhaps reverting wasm to one-encoder-per-frame (same as native) would just work — the indirect-vs-direct dispatch and the 4096-cap might be sufficient changes.

**H2: `copy_buffer_to_buffer` between rounds (Shape F) creates the needed availability barrier.** Per Vulkan spec, `vkCmdCopyBuffer` triggers `TRANSFER_WRITE` stage; the cross-stage barrier from SHADER_WRITE → TRANSFER_WRITE forces a memory availability operation that the no-op shader pass doesn't (because pure shader-shader barriers are intra-encoder cache-line-tracking only).

**H3: Algorithmic restructure — eliminate cross-pass atomic dependency on `bound_queue_sizes` entirely.** Replace the regime-2 algorithm with one that doesn't require cross-encoder atomic visibility. Specifically: have compute write a flat per-(size, axis) "did re-enqueue this round" bitmap, then have a SCAN kernel that materializes the next round's `bound_queue_sizes` from the bitmap. This bypasses the failing cross-pass atomic-add visibility.

### Plan

I'll start with H1 since it's the simplest revert. If H1 doesn't restore native-like behavior, move to H2 (copy_buffer trick). If neither works, H3 (algorithmic restructure).

I'll preserve probe1b and probe2 instrumentation throughout per the brief.

---

## Iteration 1 — H1: revert wasm to one-encoder-per-frame (mirror native path)

### Hypothesis statement

The per-round encoder+submit pattern on wasm is the cause of the cross-pass atomic invisibility, not its mitigation. By submitting each round separately, we cross a `vkQueueSubmit` boundary every round. Dawn does NOT insert a `vkCmdPipelineBarrier(SHADER_WRITE → SHADER_READ)` at the end of an encoder, and a bare `vkQueueSubmit` does NOT provide a memory-availability operation (per round-2 diagnosis D.5). If we instead put ALL 5 rounds in ONE encoder (same as native), Dawn's intra-encoder PassResourceUsageTracker will insert the proper barriers between every prepare/compute pass, and Bevy's `submit_pending_command_buffers` will submit the whole thing once per frame.

### Predict-the-outcome

If H1 is correct:
- Post-fix web `[probe1-call]` pattern shifts from "linear drain of size0_ax0 over 8 calls" to native-like "visit every (size, axis) once with found_size=32768 across ~93 calls".
- Web SSIM jumps from 0.69-0.79 cluster to ≥ 0.91.
- Native unchanged (165 calls, deterministic).

If H1 is wrong (the per-round submit IS necessary):
- Web pattern stays broken (linear drain), SSIM stays in 0.69-0.79 cluster.
- OR web pattern degrades further if the cross-submit fence was actually helping something.

### Implementation

Delete the wasm-only branch in `bounds_calc.rs:527-610`. The native path at `bounds_calc.rs:612-664` already handles regime-2 dispatch using the render_context's encoder. The wasm direct-dispatch via `compute_workgroups_override` at line 643 is preserved (separate fix for the Dawn STORAGE→INDIRECT barrier bug). The probe-2 noop pipeline stays defined but is no longer dispatched (cfg-gate fix).

### Notes before running

- Need to keep the `compute_workgroups_override` wasm path because the indirect dispatch is broken on Dawn (separate bug).
- Need to keep all the bind groups available on the native path so prepare can be set up.
- Native path already calls `dispatch_regime_2_rounds(...)` with all the right args; same call should work on wasm.

### Result

- **Web SSIM 3 runs:** 0.693755 / **PASS ≥0.91** / 0.793312
- Native: 165 probe calls deterministic, no regression.
- `[probe1-call]` pattern UNCHANGED — still linear drain `size0_ax0` 32768→4096 (8 calls), then `size0_ax1`, then `size0_ax2`, then size1, size2, ...
- Web reached size9 (run 1, 220 calls) / size8 (run 2 PASS, 195 calls) / size8 (run 3, 195 calls).
- Run 2's PASS is hard to interpret. The same logical algorithm achieved SSIM ≥ 0.91 in 195 calls, while runs 1 and 3 with 195-220 calls failed. The variability is in **how many rounds of regime-2 fire by screenshot time** combined with **how many of those rounds get the cross-pass atomic visibility to propagate before screenshot**. Some runs are lucky in the queue-rebuild rate.

### Prediction vs actual

Prediction was: if H1 correct, pattern shifts to "visit every (size, axis) once with 32768" and SSIM jumps to ≥ 0.91.
Actual: pattern unchanged (still linear drain). SSIM stochastically distributed 0.69-PASS-0.79. Run 2 PASS is consistent with "this is essentially a race; sometimes the screen captures a converged-enough state." This is NOT 3-for-3.

### Refutation analysis

H1 disproved as primary fix. Cross-encoder/cross-submit was NOT the issue — even one-encoder-per-frame the cross-pass atomic visibility is broken. The mechanism is genuinely **intra-encoder cross-pass atomic invisibility** on Dawn/Tint lowering of WGSL atomics, not a Vulkan availability barrier issue.

But H1 did NOT regress — same statistical outcome cluster as per-round-encoder. This is a good simplification, but doesn't win the probe.

### Next steps decision

Keep H1's one-encoder approach (cleaner, no per-round-submit overhead). Move to H2 — Shape F (copy_buffer_to_buffer between rounds). Per round-2 D.5, intra-encoder shader-shader barriers are insufficient. The transfer-stage barrier (TRANSFER_WRITE) is fundamentally different from SHADER_WRITE in Vulkan's pipeline-stage model. Even if Dawn's intra-encoder shader-shader barriers fail to flush, the transfer-stage barrier on the same buffer should force availability.

Wait — actually the M1 probe added an end-of-encoder no-op compute pass which the round-2 diagnosis hypothesized would force Dawn to insert a SHADER_WRITE→SHADER_READ barrier. That didn't help. So even SHADER_WRITE→SHADER_READ within one encoder doesn't fix it. The Tint lowering of atomicAdd might be fundamentally relaxed-ordering at the SPIR-V level.

This means I should pivot to a **non-atomic** approach. Or **algorithmic restructure**.

---

## Iteration 2 — H2: replace atomicAdd-into-shared-counter with a per-workgroup-per-axis-per-bound-size scoreboard, scanned later

Wait, that's a big refactor. Let me think of a smaller probe first.

Actually — a key insight from the M1-refutation: even WITHIN the same encoder, compute's atomicAdd writes to bound_queue_sizes[qi≠0] are not visible to a subsequent compute pass. So putting prepare and compute in the same encoder doesn't help cross-slot visibility within that encoder either.

Let me consider one more direct fix before refactoring algorithm: **H2 — Use `queue.write_buffer` between rounds to forcibly RESET bound_queue_sizes from compute's output mirror.** Need to mirror compute's writes... but we don't have the values.

Better: **H2 — use a host-side counter approach.** Have compute write to a different buffer (`pending_enqueues[qi]: atomic<u32>`) for the next round, then have prepare merge that into `bound_queue_sizes` itself. Both reads are now of the SAME buffer slot pattern that we know works (same-slot atomic reads survive).

Actually wait — that doesn't fix anything. The merge step is itself a cross-pass dependency.

Let me try something concrete: **H2 — Convert compute's re-enqueue logic to write to a NEW buffer (`pending_size_deltas[qi]`) that prepare merges into `bound_queue_sizes` via an `atomicAdd` operation, but the merge happens at the START of prepare in the next round.** Now prepare reads its own buffer (deltas), zeros it, and writes the merged value to sizes. The read-of-deltas vs read-of-sizes might split the broken-visibility into a different pattern.

Hmm still cross-pass.

**Better H2: Use atomic operations on a buffer that gets `copy_buffer_to_buffer`'d between rounds.** I.e., implement Shape F. Compute writes deltas to a scratch buffer; between rounds, `copy_buffer_to_buffer(scratch, bound_queue_sizes_delta_view, ...)` forces a transfer-stage barrier; prepare reads the delta view + the previous sizes and computes new sizes. The transfer-stage barrier is documented to behave differently than shader-shader.

Actually the simplest possible Shape F test: just `copy_buffer_to_buffer(bound_queue_sizes, bound_queue_sizes_self_copy, 384B)` and back, between rounds. This should force a TRANSFER stage flush.

Wait you can't copy a buffer to itself. But you can copy it to a scratch buffer and back.

Let me try this. Modify `dispatch_regime_2_rounds` to insert `copy_buffer_to_buffer(sizes_buf → scratch_buf, 384B); copy_buffer_to_buffer(scratch_buf → sizes_buf, 384B);` between the compute pass and the next prepare pass.

### Iter-2 result (n_bounds_rounds bumped to 40 on wasm)

- SSIM = 0.693738
- 1760 probe1-call lines, reached size30 (every queue!)
- Visual: web image (target/e2e-screenshots/vox_horizon_web.png) shows MUCH MORE empty space (ocean/sky) and sparser buildings vs native. NOT "rays terminating short" — rays just SKIP OVER buildings, leaving sparse coverage.
- **KEY INSIGHT**: even with the algorithm fully running every queue, web SSIM is unchanged. The bug is NOT about regime-2 convergence rate — it's about COMPUTE WRITES TO `chunks[]` BUFFER NOT PROPAGATING CROSS-PASS ON WEB.

Reverted the n_bounds_rounds bump.

---

## Iteration 3 — H3: the bug is `chunks[]` cross-pass write visibility, not `bound_queue_sizes`

### Hypothesis statement

Looking at `compute_group_bounds` WGSL body:
- Line 252: `let neighbour_x = chunks[neighbour_idx].x;` — reads NEIGHBOR chunk data via `add_bounds_group`.
- Line 538: `chunks[chunk_idx] = vec2<u32>(cur_chunk, entity_y);` — writes updated AADF to own chunk.

In round N, group G's compute may expand chunks in G via writes.
In round N+1, group G' (neighboring G) reads `chunks[neighbour].x` which falls in G.

If compute's writes to `chunks[]` from round N are NOT visible to compute's reads in round N+1 (cross-pass within or across encoders), then AADF expansion never sees the neighbor's expanded bounds and stops short. This explains:
- Web image shows partial rendering (compute did SOME expansions on chunks it processed in isolation).
- Web SSIM doesn't improve with more rounds (cross-pass reads stay stale even with more rounds).
- Web visual differs from native in HOW THE WORLD LOOKS (the rendered chunks have wrong AADF bits).

`chunks` is `array<vec2<u32>>` (non-atomic). It's a plain storage buffer rw. Each compute pass writes to it; subsequent compute passes read from it (cross-workgroup, cross-pass).

If Dawn's intra-encoder barrier for non-atomic storage-buffer writes is missing/wrong, cross-pass reads see stale data.

### Predict-the-outcome

If H3 is correct (chunks[] writes invisible cross-pass on web):
- Forcing a barrier or staleness-flush on `chunks[]` between rounds should fix the rendering.
- Web SSIM jumps to ≥ 0.91 in all 3 runs.
- `[probe1-call]` pattern unchanged (probe is about bound_queue_sizes which is a different buffer).

If H3 is wrong:
- No improvement from chunks-barrier intervention.

### Probe approach

Use `copy_buffer_to_buffer` self-copy on chunks buffer between rounds to force a TRANSFER-stage barrier. The chunks buffer is large (16 MiB) so copy cost is non-trivial; could use a small subset copy if cost is an issue.

Actually a SIMPLER idea: try Shape F directly on the chunks buffer. Between rounds insert `copy_buffer_to_buffer(chunks, chunks_scratch, 16 MiB)`. The TRANSFER stage barrier should flush all of chunks's writes to L2/global, making them visible to the next compute pass's reads.

Wait — there might be an even simpler fix. **What if compute_group_bounds had a `storageBarrier()` somewhere?** The current shader at line 528 has `workgroupBarrier()` but no `storageBarrier()`. Let me check WGSL semantics — `storageBarrier()` is workgroup-scope per `WGSL §17.11.1`, so it doesn't help cross-workgroup.

We need DEVICE-scope barrier. WGSL has none. Need to do it host-side.

### Plan

Add a `copy_buffer_to_buffer` of the chunks buffer between rounds on wasm. If chunks is 16 MiB, copy cost ~few ms per round, 5 rounds × 5 ms = 25 ms/frame — too expensive. Need a smaller copy, OR a different mechanism.

Actually, MAYBE we don't need to copy the whole chunks buffer. Maybe just copying a SCRATCH buffer of any kind triggers a global memory flush. Let me try a tiny scratch buffer copy.

Actually, even simpler: ANOTHER compute pass that just touches chunks could be enough. The M1 noop touched `bound_queue_sizes`, which is a DIFFERENT buffer than chunks. The noop made Dawn insert a barrier on `bound_queue_sizes`, but not on `chunks`. **A noop touching `chunks` rw might insert the right barrier**.

But hold on — the same `compute_group_bounds` pass writes chunks AND reads chunks. So within the same pass, there's a self-dependency. The next round's compute pass also has the same self-dependency. Dawn intra-encoder should insert a barrier between compute pass N's chunks-write and compute pass N+1's chunks-read.

But just like with bound_queue_sizes (per M1), Dawn's intra-encoder barrier on storage writes might be insufficient for cross-workgroup write→read.

So the question becomes: WHY does this work on native and not on web?

Actually I realized I should test my hypothesis directly first. Let me run iter-3 with a copy_buffer_to_buffer of chunks self-copy to a scratch buffer between rounds.

### Iter-3 results

Added `chunks[0]` atomicLoad+atomicStore to the end_of_encoder_noop entry point (so the noop also touches chunks, forcing Dawn's tracker to insert SHADER_WRITE→SHADER_READ barrier on chunks too).

- Web SSIM: 0.81, 0.69, 0.69 (1 marginal improvement, mostly broken)
- Pattern unchanged, no qualitative shift

H3 refuted. Adding a shader-side touch of chunks didn't change anything materially.

### Iter-4 results

Inserted `copy_buffer_to_buffer(chunks_buffer, indirect_buffer, 4)` between compute pass and end_of_encoder_noop on wasm. This forces Dawn to emit a TRANSFER stage barrier on chunks — a strictly stronger memory availability operation than SHADER_WRITE→SHADER_READ.

- Web SSIM: 0.69, 0.69, PASS (one passing run as luck, mostly broken)
- Same statistical cluster as iter-1/iter-3.
- Initial probe shows web chunks[chunk@(242,31,219)] = 0x00000000 (all bits zero) — different from native [4,4,3,3,3,3] startup state. Could be timing issue (probe fires before W1 writes propagate) or could indicate W1's chunk_calc output also has a propagation issue.

H4 refuted. TRANSFER-stage barrier doesn't fix the issue either.

### Iter-5 results — combined approach

Combined: per-round-encoder+submit + copy_buffer_to_buffer(chunks) + n_bounds_rounds=40 (8× the per-frame round count to compensate for the 8× smaller per-round claim on wasm).

- Web SSIM: 0.69, 0.69, 0.69 — three for three FAILED.
- ACTUALLY WORSE than baseline (run 2 of iter-1 had passed). The interventions are not helping.
- The combined approach didn't restore the random PASS — quite the opposite.

### Visual observation re-confirmed

iter-4 web-run-3 PASSED with image visually nearly identical to native (dense buildings, ocean horizon line). FAILED runs show sparser buildings (visible in `target/e2e-screenshots/vox_horizon_web.png`). The algorithm CAN produce native-quality output sometimes; the question is making it ALWAYS produce native-quality output.

---

## STATUS: BAILED — iteration 5 exhausted ideas

### Hypotheses tried in this dispatch

1. **H1 (iter-1): Revert wasm to one-encoder-per-frame (mirror native path).** Web SSIM 0.69/PASS/0.79. Pattern unchanged. Per-round submit boundary is NOT the cause — even with one encoder, cross-pass atomic visibility is still broken on web. Statistical outcome stochastically distributed.

2. **H2 (iter-2): Raise n_bounds_rounds to 40 on wasm.** Web SSIM 0.69. Reaches every size level in 1760 calls (full algorithmic convergence) yet SSIM unchanged. CRITICAL FINDING: convergence rate is NOT the bug. Even with full queue convergence, the rendered chunks are wrong.

3. **H3 (iter-3): Add `chunks[0]` read+write to end_of_encoder_noop pipeline.** Web SSIM 0.81/0.69/0.69. Marginal improvement only. The SHADER-side barrier on chunks doesn't fix it.

4. **H4 (iter-4): `copy_buffer_to_buffer(chunks, scratch, 4)` between rounds.** Web SSIM 0.69/0.69/PASS. TRANSFER-stage barrier didn't help. Same stochastic cluster.

5. **H5 (iter-5): Combine H1+H4+H2 (one-encoder + chunks-copy + n_bounds_rounds=40).** Web SSIM 0.69/0.69/0.69 — three for three failed. Worse than baseline. Combining interventions does NOT additively help; possibly the bigger n_bounds_rounds is exhausting some Dawn resource limit or hitting a Chrome watchdog.

### Critical finding (load-bearing for next dispatch)

The probe-1B telemetry shows the algorithm CAN reach every queue (size level 0..30) on web. iter-2 with n_bounds_rounds=40 produced 1760 prepare calls, reaching all queues. Yet SSIM stayed 0.69. **This refutes ALL hypotheses based on "cross-pass atomic visibility breaks regime-2 convergence."** The W3 IS converging on web, it just produces a different visual result.

Inspecting the screenshots:
- `target/e2e-screenshots/vox_horizon_native.png` shows dense buildings (correct).
- `target/e2e-screenshots/vox_horizon_web.png` from FAILED runs shows sparser buildings (skipping over geometry).
- `target/e2e-screenshots/vox_horizon_web.png` from PASS runs shows nearly identical to native.

So the bug is **stochastic image quality**, not algorithmic non-convergence. The probe data CONFIRMED bound_queue_sizes get drained, BUT this is happening on EVERY run regardless of SSIM outcome. So the queue-state is not the discriminator between PASS and FAIL.

What IS different between PASS and FAIL on web is unclear. May be:
- Timing of W1 chunks-data propagation vs renderer
- Whether some specific (chunks, blocks, voxels) writes "land" before screenshot
- Dawn's batched-flush threshold being hit

### Ideas considered but not tried (and why)

1. **Algorithmic restructure (Shape G — queue rebuild from masks).** Major refactor; the data shows the existing algorithm CAN produce correct output (PASS runs). Restructure may not help if the bug is at a lower level than the algorithm.

2. **CPU read-back of chunks+bounds_queue between rounds (Shape C).** Documented in fix-design as "structurally a non-starter" due to map_async latency on web. Probably not worth trying.

3. **Disabling GPU construction on wasm (forcing CPU fallback).** Bypasses the entire GPU compute chain. Would work but is a major regression of GPU-acceleration intent.

4. **Pre-baking AADF on CPU and uploading once.** Same as #3 conceptually.

5. **Pure WGSL `storageBarrier()` between dispatches.** WGSL only exposes workgroup-scope barriers, not cross-pass barriers. Can't be done in WGSL alone.

6. **Tint version pinning / Chrome version testing.** Out of scope — Chrome stable is the user's target.

### Hard constraints encountered

- Cannot raise `WASM_MAX_GROUP_BOUND_DISPATCH` (forbidden by handoff).
- Cannot lower SSIM floor (forbidden).
- Cannot raise `MAX_RAY_STEPS_PRIMARY` (forbidden).
- Cannot commit / push (forbidden until orchestrator's go-ahead).

### Recommended directions for the next dispatch

**High priority — re-frame the investigation.** The data strongly suggests the bug is NOT what the round-1 and round-2 diagnoses identified. The probe-1B telemetry empirically proved (in iter-2) that bound_queue_sizes IS reaching all 32 size levels on web. Yet SSIM stays broken. The diagnoses were anchored on a hypothesis that's empirically refuted.

The next investigator should:

1. **Look elsewhere than `bound_queue_sizes`.** The bug isn't W3 regime-2 atomic visibility. The bug is somewhere in:
   - The renderer (`ray_tracing.wgsl` reads `chunks[]`, `blocks[]`, `voxels[]`)
   - The W1 chunk_calc (`compute_voxel_bounds` / `compute_block_bounds`)
   - The W2 world_change (probably not — gate has no edits)
   - Material data upload
   - The actual chunks data being uploaded

2. **Compare native vs web `chunks[]` contents at a specific chunk index after settle.** Add a probe that reads back `chunks[bound_idx]` after the SSIM-screenshot moment on both targets and dumps the value. Compare. If they differ, the bug is in the construction chain. If they MATCH, the bug is in the renderer.

3. **Bisect the construction chain on wasm.** Disable W3 on wasm (skip bounds_calc). See if SSIM improves to PASS consistently. If yes, W3 is at fault. If no, look at W1.

4. **Test with `gpu_construction_enabled = false` on wasm.** Forces CPU construction. If web SSIM consistently PASSes with CPU construction, the bug is definitively in the GPU compute chain. If still broken, the bug is in the renderer or upload path.

5. **Compare aadf-probe lines from probe2 (post-convergence) between native and web** for the SAME chunk indices. If the bound values match but SSIM diverges, the bug is post-W3.

6. **Investigate the chunks-data-upload path.** `parse_cvox` produces CPU mirror. That mirror is uploaded via `write_buffer` to chunks buffer. Maybe the upload path on wasm is dropping data or padding incorrectly.

### State of the worktree at bail time

Modified files (current state — iter-5 combined approach in place):
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs` — wasm path uses per-round-encoder+submit with chunks-copy_buffer_to_buffer intervention between compute and noop pass.
- `crates/bevy_naadf/src/render/construction/config.rs` — wasm `n_bounds_rounds` clamped UP to 40.
- `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` — end_of_encoder_noop body unchanged from iter-2 baseline (reverted iter-3 chunks-touch).

New files: none beyond IMPLEMENTORS_SHARED.md.

The orchestrator should consider reverting `bounds_calc.rs` and `config.rs` to the pre-iter-1 state (per-round-encoder+submit, n_bounds_rounds=5, no chunks-copy_buffer_to_buffer) before the next dispatch — the iter-5 combined state shows WORSE statistical outcomes (3/3 FAIL) than the original (1/3 PASS).

### Logs / artefacts

All per-iteration logs in `target/diagnostics/consolidated/iter-{1,2,3,4,5}/`:
- `web-run-{1,2,3}.log` — full Playwright stdout with SSIM, panic markers, [probe1-call] lines.
- `native-run-{1,2}.log` — native e2e_render output for sanity checking.
- `web-build.log` — wasm release build output.

Visual diff: screenshot pairs at `target/e2e-screenshots/vox_horizon_{native,web}.png`. The web image (from iter-4 last run which PASSED) is nearly identical to native, demonstrating that the algorithm CAN produce correct output when the stars align. The challenge is making it ALWAYS produce correct output.

