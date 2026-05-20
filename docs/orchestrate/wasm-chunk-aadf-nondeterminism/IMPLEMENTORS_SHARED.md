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


---

## Dispatch 2 — second implementor

### Context loaded on entry
- IMPLEMENTORS_SHARED.md (full, dispatch-1 trace).
- 00-handoff-verbatim.md (full).
- 02-diagnostics-impl.md (84-divergence catalog + 6 load-bearing).
- 03-diagnosis.md, 04-probe1-impl.md, 05-fix-design.md, 06-fix-impl.md, 07-diagnosis-round2.md, 08-probe2-impl.md.
- Read source: `bounds_calc.rs` (711 L), `bounds_calc.wgsl` (579 L), `config.rs:200-289`, `world_data.wgsl` (renderer's read-only chunks binding), `mod.rs:1093-1565` (populate_cpu_mirror), `mod.rs:300-330` (Core3d schedule chain).

### Decisions about prior dispatch's worktree state
- **Reverted iter-5's `n_bounds_rounds = max(40)` config bump** (config.rs:241-248). iter-2 already proved algorithmic convergence is NOT the bug.
- **Reverted iter-4's chunks `copy_buffer_to_buffer(chunks, indirect, 4)` between rounds** (bounds_calc.rs:592-608). Refuted by iter-4 results.
- **KEPT the end-of-encoder noop pass** (M1 probe) — leaving instrumentation in place per brief.
- **KEPT per-round encoder + submit** (H1 from iter-1) — neutral to baseline statistically and cleaner separation between rounds.
- **KEPT probe1-call (probe-1B) instrumentation everywhere** — load-bearing.

After reverts, baseline = per-round-encoder+submit + M1 noop, n_bounds_rounds=5 (native default), max_group_bound_dispatch=4096 wasm-clamp.

### Dual-angle enumeration (orchestrator's mandate)

**Angle A — "why broken on web"** (prior dispatches focused here):
- A1. Tint lowers atomicAdd to memory model with missing Coherent decoration → cross-pass atomic invisibility. REFUTED by iter-2 (queue sizes DO drain, all 32 levels reached).
- A2. Dawn intra-encoder shader→shader barrier insufficient → cross-pass storage write→read invisibility on `chunks[]`. REFUTED by iter-3+iter-4 (no improvement from TRANSFER-stage barrier or shader-side touch).
- A3. Renderer reads `chunks[]` via world_data.wgsl binding before W3's writes land — Dawn missing cross-NODE / cross-submit barrier between W3's separate-encoder submits and the main render encoder. **NOT YET PROBED.**
- A4. The W3 wasm code uses `render_device.create_command_encoder()` + `render_queue.submit()` directly. The renderer uses Bevy's `render_context.command_encoder()` (one big encoder). The two encoders are NOT in the same submit; submits are ordered W3-1..W3-N then main, but the main encoder's bind groups reference `chunks` as RO. Dawn must insert resource-tracking barrier ACROSS submits — empirically, this may be working sometimes (rare-success) and not other times (broken-side).
- A5. The bind group for `world_data` (renderer) is built ONCE in `prepare`. After W3 writes to `chunks_buffer`, the bind group is stale — but bind groups reference buffer HANDLES (GPU-side), not snapshots. So this should be live. CHECK: is there a render-side bind group that's built from a cache that's older than W3's first write?
- A6. **subgroup_min/max_size DIVERGES (native 32/32, web 4/128)** — the WebGPU adapter exposes wider subgroup range. If the compute_group_bounds workgroup (4³=64 threads) gets dispatched on subgroups of size 4 on web (vs 32 native), thread synchronization semantics differ. `workgroupBarrier()` is workgroup-scoped, NOT subgroup-scoped — should be fine. But uniform control flow assumptions might differ.
- A7. **max_dynamic_storage_buffers_per_pipeline_layout: 8 on web vs 16 on native.** prepare_group_bounds binds 7 storage buffers in 4 groups. Could be at a cliff that triggers different shader-compilation paths.
- A8. **max_bind_groups: 4 on web vs 8 on native.** prepare_group_bounds uses exactly 4 groups. Recompiling tight to web's exact cap might trigger different layout allocation on the GPU side.
- A9. **max_buffer_size: 4 GiB - 4 on web vs 1 TiB on native.** Not a problem for current buffer sizes but worth tagging.
- A10. **min_storage_buffer_offset_alignment: 256 on web vs 32 on native.** Could affect dynamic-offset binding alignment — but we don't use dynamic offsets in W3.

**Angle B — "why does it OCCASIONALLY work"** (orchestrator's new emphasis):
- B1. Race in submit ordering — sometimes W3's last submit lands before main encoder's submit, sometimes not. Browser microtask scheduling jitter.
- B2. Dawn maintains a batched flush threshold (e.g. N submits or M bytes pending); when the threshold is crossed, all pending writes are flushed including stale W3 chunks-writes. If a frame happens to cross the threshold mid-W3 (lucky), data lands in time for renderer; otherwise it sits in the Dawn driver's deferred-flush queue.
- B3. GPU warmup state — first few frames after page load have cold caches / pipeline-compilation pauses that ACCIDENTALLY serialize W3 writes to global memory before the renderer runs.
- B4. The `populate_cpu_mirror_from_gpu_producer` issues a `copy_buffer_to_buffer(chunks_buffer, staging, ...)` and a `queue.submit([enc.finish()])` (mod.rs:1200-1218). This copy+submit happens EARLY in the frame (ExtractSchedule). On the rare "lucky" run, this submit may serve as a TRANSFER barrier that flushes chunks writes from prior frame, making the renderer's read see correct data this frame. On unlucky runs, the readback runs once after first GPU producer pass, then never re-fires (gated on `cpu_mirror_populated`), so W3's per-frame writes never get an explicit transfer-barrier.
- B5. (Combination of A4 + B4): the readback-driven copy_buffer_to_buffer happening ONCE early in the frame acts as a one-time barrier. If renderer reads chunks before W3 has expanded the AADF beyond a threshold, the rays terminate short. If by luck enough W3 rounds have accumulated to global memory in the readback flush, the rays go further. Once `cpu_mirror_populated` flips true, the readback-flush is gone; whatever state was captured then is what the renderer sees.

**Hypotheses ranked combining both angles:**

**H1 (HIGH PRIORITY, combination): A4 + B4 + B5 — renderer's bind group reads stale `chunks[]` because Bevy's render-graph submits the main encoder BEFORE Dawn has flushed W3's separate per-round submits to global memory.** A barrier ACROSS submits should be implicit but Dawn may be relaxed about it for storage buffers that change usage class (W3 writes RW, renderer reads RO from same buffer, distinct bind groups). The rare-success matches B4: the `populate_cpu_mirror_from_gpu_producer`'s copy_buffer_to_buffer (early in frame, before W3) acts as a one-time TRANSFER barrier that on a lucky frame lands chunks state correctly before the renderer runs.

Fix attempt: insert a per-frame `copy_buffer_to_buffer(chunks_buffer, chunks_buffer, 4)` (or chunks→indirect 4B) AFTER W3's last round on wasm. This forces a TRANSFER barrier on chunks AFTER all W3 writes are issued, which Dawn should chain before the next submit (the main encoder). This is structurally different from iter-4 which inserted copies BETWEEN rounds — H1 here inserts the copy ONCE at end-of-W3 (or just submits a dedicated transfer-only encoder after W3's last round).

**H2 (MEDIUM PRIORITY, broken-side only): A5 — the renderer's world_data bind group is built using a CACHED chunks_buffer handle that's not the live W3-target buffer.** Sometimes the bind group gets rebuilt by some other system (lucky case). CHECK: source path that builds `naadf_world_bind_group` and verify it references `world_gpu.chunks_buffer` directly.

**H3 (MEDIUM PRIORITY, combination): A6/A7/A8/A10 — some capability divergence forces Dawn to lower compute_group_bounds shader differently on web (e.g. with relaxed atomic memory ordering, or with a per-subgroup loop rewrite).** Hard to test without re-instrumenting; deferring unless H1/H2 fail.

**H4 (REJECTED for now): Algorithmic restructure (Shape G).** iter-2 of dispatch-1 proved algorithmic convergence happens on web. Restructure won't help.

### Plan for iteration

Iter-2-1: Verify baseline (post-reverts) and confirm the iter-1 statistical cluster (0.69-0.79-PASS).
Iter-2-2: H1 — `copy_buffer_to_buffer(chunks, scratch, full-size or 4B)` AFTER W3's last round, ONCE per frame on wasm. Predict: web SSIM stabilizes at PASS if H1 is correct. Probe-1B pattern unchanged (probe is about queue_sizes, H1 fix is about cross-submit chunks barrier).

---

## Iteration 2-1 — baseline after reverts (per-round encoder + M1 noop)

### Setup
- Reverted iter-5's `n_bounds_rounds=max(40)` → 5 (native default).
- Reverted iter-4's chunks copy_buffer_to_buffer between rounds.
- Kept per-round encoder + render_queue.submit + M1 noop pass.

### Results
- Web SSIM: 0.793145 / 0.792506 / 0.693035 (cluster 0.69-0.79, baseline)
- Web AADF at chunk@(242,31,219): `word=0x02100000 chunk_aadf=[mx=0 px=0 my=0 py=0 mz=1 pz=1]`
- Native AADF: `word=0x06318c84 chunk_aadf=[mx=4 px=4 my=3 py=3 mz=3 pz=3]`

**CRITICAL OBSERVATION:** On web, EMPTY chunks show AADF=0,0,0,0,1,1 (only Z-axis expansion by 1). On native, AADF=4,4,3,3,3,3 (multi-round expansion across all axes).

The bug: **cross-pass writes to `chunks[chunk_idx]` from `compute_group_bounds` get OVERWRITTEN by subsequent rounds' reads of stale chunks=0**. Each round of W3 reads chunks (sees 0), expands one axis by 1, writes back. Next round reads chunks again — sees 0 again (or sees a different round's write) — expands its own axis by 1, OVERWRITES previous expansion. Only the LAST round's expansion survives.

This matches the symptom: on web, AADF ends up with 1 bit on only ONE axis (the last-processed axis).

---

## Iteration 2-2 — H1: native code path on wasm (one encoder, indirect dispatch)

### Hypothesis
The wasm-only per-round-encoder+submit pattern was the cause. Bevy's main encoder (`render_context.command_encoder()`) puts W3 + renderer in ONE command buffer; Dawn's intra-encoder PassResourceUsageTracker should insert the chunks SHADER_RW→SHADER_READ barrier properly. Removed the wasm-specific path so wasm uses `dispatch_regime_2_rounds` exactly like native, with indirect dispatch.

### Predicted outcome
Web SSIM jumps to ≥0.91 if the cross-encoder pattern was the cause.

### Results
- Web SSIM: 0.793149 / 0.693496 / 0.693788 (same cluster)
- Web AADF: `word=0x02100000 chunk_aadf=[mx=0 px=0 my=0 py=0 mz=1 pz=1]` (same as baseline)
- **Probe-1B**: `found_size=32768` initially drains by 4096 per call — meaning indirect dispatch IS reading post-prepare bound_dispatch_indirect[0]=4096. So Dawn's STORAGE→INDIRECT barrier IS working. The handoff's "indirect-barrier-broken" claim is REFUTED in this state.

### Refutation analysis
H1 wrong. Putting W3 in Bevy's main encoder with indirect dispatch didn't fix it. Direct-vs-indirect is NOT the discriminator. Encoder boundary is NOT the discriminator. The bug is INTRA-ENCODER cross-pass chunks-write visibility.

---

## Iteration 2-3 — H2: flip renderer's chunks binding to read_write

### Hypothesis
Dawn fails to insert the SHADER_RW→SHADER_READ usage transition barrier on chunks_buffer between W3 (storage_rw binding) and renderer (storage_read binding). Forcing both bindings to read_write makes them identical-usage-class and removes the transition.

### Predicted outcome
If usage-transition is the cause, web SSIM jumps to ≥0.91.

### Results
- Web SSIM: 0.693419 / 0.693632 / 0.694291 (3/3 LOWEST, regressed vs baseline)
- Web AADF: `word=0x00000000 chunk_aadf=[0,0,0,0,0,0]` (REGRESSED to all-zeros)

### Refutation analysis
H2 wrong AND regressing. Making chunks rw on renderer side made things worse — possibly because Dawn then thinks the renderer ALSO writes chunks, leading to a different stall pattern. **REVERTED.**

---

## Iteration 2-4 — H4-redo: FULL chunks self-copy between rounds via scratch buffer

### Hypothesis
Dispatch-1 iter-4 inserted `copy_buffer_to_buffer(chunks, scratch, 4 bytes)` which may have only barriered the 0-3 byte range. Re-test with FULL chunks-size (16 MiB) copies, AND additionally copy back into chunks (forcing the chunks usage to transition both into and out of TRANSFER).

### Implementation
- Allocated lazy `chunks_self_copy_scratch` buffer (16 MiB, COPY_SRC|COPY_DST) via `Local<Option<Buffer>>` on the W3 system.
- Between each round of {prepare, compute}:
  - `encoder.copy_buffer_to_buffer(chunks, 0, scratch, 0, chunks.size())`
  - `encoder.copy_buffer_to_buffer(scratch, 0, chunks, 0, chunks.size())`
- Used Bevy's main `render_context.command_encoder()` so W3 + renderer share one cmd buffer.
- Kept direct-dispatch + 4096 cap on wasm.

### Predicted outcome
If chunks cross-pass write visibility IS the bug AND copy_buffer_to_buffer barrier scope is per-byte-range (not whole-buffer), then full-size copies should restore visibility → SSIM ≥0.91.

### Results
- Web SSIM: 0.809535 / 0.693448 / 0.694741
- Web AADF run-1: `chunk_aadf=[mx=0 px=0 my=1 py=1 mz=1 pz=1]` (FIRST TIME 4 axes expanded! though only by 1 each, native does 3-4 rounds)
- Web AADF run-3: `chunk_aadf=[0,0,0,0,0,0]` (REGRESSED to no expansion)
- Run-1 SSIM=0.810 is the **HIGHEST DISPATCH-2 SCORE** (vs prior cluster 0.69-0.79).

### Partial confirmation, not enough to win

H4-redo gives strongest evidence yet that the cross-pass chunks visibility IS the bug:
- Best run shows Y and Z and pz expansion landing (4 of 6 axes got 1 bit).
- Worst run shows total regression (no expansion).

But: even FULL chunks copies don't reliably make ALL rounds' writes propagate. The native pattern is "expand by 1 per round per axis, across multiple rounds" (final 3-4 per axis). Web gets only 1 round of expansion per axis at best, never multi-round.

This suggests that **even with full chunks copies, Dawn ISN'T making cross-pass writes from compute → next compute visible reliably**. Possible reasons:
- Dawn batches copy_buffer_to_buffer with subsequent compute passes without intermediate sync.
- The compute pass's SHADER_WRITE→COPY_SRC barrier doesn't actually flush to global memory; it just changes usage tracking.
- WebGPU's spec is non-binding about this; Chrome's Dawn implementation defers.

### State at iteration 2-4 end
- Modified files: 
  - `bounds_calc.rs` — wasm uses render_context.command_encoder() with per-round chunks self-copy intervention via lazy Local<Option<Buffer>>. Native path unchanged.
  - `config.rs` — reverted iter-5's n_bounds_rounds=40 bump.
- `world_data.wgsl` and `pipelines.rs` — reverted iter-2-3's read_write flip.

---

## STATUS: BAILED (dispatch 2) — iteration 4 exhausted

### Hypotheses tried in dispatch-2 (with angle classification)

1. **iter-2-1 baseline**: revert iter-5 and iter-4 to per-round-encoder + M1 noop. angle=broken-side. SSIM=0.793/0.793/0.693. Confirmed baseline cluster. Web AADF only 1 bit on Z axis vs native's 4,4,3,3,3,3.
2. **iter-2-2 H1**: native code path on wasm (one encoder, indirect dispatch). angle=combination. SSIM=0.793/0.693/0.694. Encoder boundary NOT the issue. Indirect IS working (barrier OK).
3. **iter-2-3 H2**: chunks read_write on renderer. angle=broken-side (usage-transition theory). SSIM=0.693/0.693/0.694. REGRESSED. REFUTED + REVERTED.
4. **iter-2-4 H4-redo**: FULL chunks self-copy between rounds. angle=combination. SSIM=0.810/0.693/0.695. BEST RESULT so far (0.810 is highest dispatch-2 SSIM); web AADF for the GOOD run shows 4/6 axes expanded (vs only 1 in baseline). Partial confirmation that cross-pass chunks visibility is the bug. But not 3/3 PASS.

### Critical finding for next dispatch

The bug is **cross-pass write→read visibility on `chunks[]` storage buffer on Dawn/WebGPU**. Each round of `compute_group_bounds` reads `chunks[chunk_idx]`, computes new AADF, writes back. On native, subsequent rounds read the FRESH value. On web, subsequent rounds read STALE value (0) and overwrite. Result: only the LAST round's writes survive, giving AADF=0,0,0,0,1,1 (only Z, the last-processed axis).

This is NOT solved by:
- per-round-encoder+submit (dispatch-1 iter-0, iter-2-1)
- one-encoder-per-frame (dispatch-1 iter-1, iter-2-2)
- end-of-encoder noop (dispatch-1 probe-2 / iter-2-1)
- chunks copy_buffer_to_buffer(4 bytes) between rounds (dispatch-1 iter-4)
- chunks copy_buffer_to_buffer(FULL size, both directions) between rounds (dispatch-2 iter-2-4) — PARTIAL
- renderer chunks binding flipped to rw (dispatch-2 iter-2-3) — REGRESSED

### Ideas considered but not tried this dispatch

1. **Algorithmic restructure (Shape G — queue rebuild from masks).** Big refactor; the data shows the existing algorithm WORKS on native and FAILS on web for the same reason. Restructure doesn't address the GPU memory visibility bug.

2. **Read+write all chunks via single-pass, no cross-pass dependency.** Have ONE compute pass do ALL axes for ALL queues for ALL bound sizes in one go. Impossible: bound size N requires the result of bound size N-1.

3. **Read chunks ONLY ONCE per frame, cache in workgroup memory.** Doesn't scale (workgroup memory ~64 KB, chunks ~16 MB).

4. **Use storageBarrier() at end of compute_group_bounds.** WGSL storageBarrier is workgroup-scope, not cross-workgroup. May or may not help; not tested due to running out of iteration budget.

5. **Bisect via custom probe: a one-shot e2e test that reads chunks[chunk_idx] from a known position AFTER all W3 work, compare native vs web verbatim.** Indirectly done via the aadf-probe2 readback; both targets show their respective AADF values.

6. **Eliminate the cross-pass dependency by passing AADF state through bound_group_queues instead of chunks.** Each entry in bound_group_queues carries (group_pos, current_AADF). Compute_group_bounds reads current AADF from queue entry, updates it, writes back to chunks (final). bound_group_queues is then the actual mutable state across rounds — and it IS atomic, so it might survive the cross-pass visibility issue.

7. **Stall the screenshot until the algorithm has been "running long enough" by polling some convergence flag.** Doesn't help if writes never propagate.

8. **Use a different shader-compile path on wasm.** E.g., `naga::Backend::Wgsl` instead of `naga::Backend::Spv`. Not in scope.

### Recommended directions for the next dispatch (dispatch 3)

**High priority — restructure data flow to avoid cross-pass chunks dependency.**

Option A — **Queue carries AADF state**: extend bound_group_queues entries from `u32` (packed position) to `vec2<u32>` (packed_position, current_aadf). compute_group_bounds reads its AADF from the queue entry (not chunks), expands, writes back to the queue AND optionally to chunks. The renderer reads chunks (the cached final state).

Option B — **Per-round chunks subset writes**: limit each compute pass to ONLY the chunks for the CURRENT bound size + axis combination. The other axes' chunks are written by OTHER compute passes. Since each axis writes to disjoint chunk-axes-bits within the SAME chunk word, writes from different rounds DON'T conflict if interleaved correctly. But the AADF axis bits live in the SAME word, so this isn't possible without atomicOr.

Option C — **Use atomicOr on chunks**: convert chunks `array<vec2<u32>>` to `array<atomic<u32>>` (drop .y, use a separate buffer for entity pointer). Each `compute_group_bounds` round uses `atomicOr(&chunks[chunk_idx], 1u << bounds_location)` to set the AADF bit. atomicOr survives cross-pass visibility issues better than non-atomic writes (per dispatch-1 H2 / iter-2 evidence on bound_queue_sizes which works as atomic). This is a significant refactor of bounds_calc.wgsl + chunk_calc.wgsl + ray_tracing.wgsl + world_data.wgsl, but bounded.

Option D — **CPU fallback for W3 on wasm**: gate `gpu_construction_enabled = false` on wasm so the CPU `construct()` builds AADF on CPU and uploads chunks once. Loses GPU-acceleration but provides correctness. Easy.

**LOW priority — punt on PR-completeness, ship as-is with `cpu_fallback=true` on wasm.** The handoff is months-deep; the orchestrator might prefer a working wasm build over a fast wasm build.

### State of the worktree at bail time

Modified files (current state — iter-2-4 chunks-self-copy in place):
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs` — wasm uses `render_context.command_encoder()` with per-round chunks self-copy via lazy `Local<Option<Buffer>>`. The native path is unchanged.
- `crates/bevy_naadf/src/render/construction/config.rs` — reverted iter-5's `n_bounds_rounds = max(40)` to native default (5).

Files reverted from prior dispatch's edits:
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs` — removed the per-round-encoder+submit + chunks-copy(4byte) state from dispatch-1 iter-5.
- `crates/bevy_naadf/src/assets/shaders/world_data.wgsl` — re-verted iter-2-3's `read_write` flip; back to `read`.
- `crates/bevy_naadf/src/render/pipelines.rs` — re-verted iter-2-3's storage_buffer_sized flip; back to storage_buffer_read_only_sized.

Files NOT touched in dispatch-2:
- `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` — unchanged from end of dispatch-1.

Probe-2's end_of_encoder_noop dispatch is REMOVED in dispatch-2's iter-2-4 (we no longer use the per-round-encoder branch). The PIPELINE resolution is still gated on wasm (line 515-522) but the dispatch is suppressed via `let _ = end_of_encoder_noop_pipeline;` at line 605.

### Logs / artefacts

All per-iteration logs in `target/diagnostics/consolidated/iter-2-{1,2,3,4}/`:
- `web-run-{1,2,3}.log` — full Playwright stdout with SSIM, panic markers, [probe1-call] lines, [aadf-probe] AADF samples.
- `native-run-1.log` (iter-2-1 only) — native run with [aadf-probe2] [probe1-call] convergence pattern.
- `web-build.log` — wasm release build output.

Visual diff: screenshot pairs at `target/e2e-screenshots/vox_horizon_{native,web}.png`. The latest web image reflects iter-2-4's chunks-self-copy state.

### Recommendations for orchestrator

**Strongly suggest dispatch-3 attempt Option C (chunks as atomic<u32>) before Option D (CPU fallback).** The data from iter-2-4 shows chunks-write-visibility IS the bug; atomic operations have been shown to work cross-pass on web (via the `bound_queue_sizes` split-buffer fix). Converting chunks to atomic should follow the same proven pattern. Cost: ~3 days of refactoring across 4 shaders + their Rust bindings + their layout descriptors.

If Option C also fails, drop to Option D (CPU fallback) as a known-good escape hatch.


---

## Dispatch 3 — third implementor

### Context loaded on entry
- IMPLEMENTORS_SHARED.md (full, dispatch-1 + dispatch-2 traces, 585 lines).
- 09-synthesis-classifications.md (full, the new top-candidate analysis).
- 00-handoff-verbatim.md (full).
- Re-verified source line numbers for chunks-access in bounds_calc.wgsl:
  - `chunks[neighbour_idx].x` at line 252 (in `add_bounds_group` helper)
  - `let cur_chunk_full = chunks[chunk_idx];` at line 499 (RMW read)
  - `chunks[chunk_idx] = vec2<u32>(cur_chunk, entity_y);` at line 538 (RMW write)
  All cited correctly in synthesis §F.
- Verified worktree state: `bounds_calc.rs` carries dispatch-2 iter-2-4's
  chunks-self-copy pattern (chunks->scratch->chunks BETWEEN rounds, inside
  Bevy's main encoder). `config.rs` has n_bounds_rounds at native default (5).
  end_of_encoder_noop pipeline still defined but suppressed.

### Plan for dispatch-3

Per synthesis §G:
- Iter 1: cheap fence probe (queue.on_submitted_work_done equivalent + dedicated W3 submit). Discriminates C3/C6 (fence-family bug) from C1/C4 (intrinsic RMW race on chunks[]).
- Iter 2: chunks history probe if iter 1 doesn't win.
- Iter 3+: based on data.

---

## Iteration 3-1 — Probe 1 (cheap fence probe)

### Hypothesis statement (C3 + C6 family)

The bug is that W3's compute writes to `chunks[]` are batched in Bevy's main
encoder along with everything else, submitted in one `queue.submit` call at
end of frame; Dawn either (a) does not insert the necessary
SHADER_WRITE->SHADER_READ barrier across the W3 compute pass and the
renderer's later read in the SAME command-buffer-batch submit, or (b) defers
its flush such that the renderer's reads see stale chunks state on most
frames, with occasional "lucky" runs where the flush coincides.

If we (i) pull W3's dispatches out of Bevy's main encoder onto a DEDICATED
encoder, (ii) issue `queue.submit([w3_encoder.finish()])` IMMEDIATELY (its
own submission boundary BEFORE Bevy's main submit), (iii) force Dawn to
materialize that submit via `queue.on_submitted_work_done(callback)` + a
trivial `map_async` on a 4-byte scratch buffer that received a
copy_buffer_to_buffer(chunks, scratch, 4) inside the same dedicated encoder,
then Dawn must flush W3's writes to global memory before Bevy's main encoder
submit picks up. Renderer reads chunks via its own bind group; the cross-
submit dependency on chunks SHOULD then be honoured.

### Predict-the-outcome

If C3/C6 (fence-family) is correct:
- Web SSIM stabilises at >=0.91 on all 3 runs.
- Web AADF probe value shifts from `[0,0,0,0,1,1]` toward
  native's `[4,4,3,3,3,3]` (multi-axis multi-round expansion visible).
- [probe1-call] pattern may also shift to "every (size, axis) visited once"
  resembling native, because cross-pass `bound_queue_sizes` reads now see
  fresh writes from the prior compute's atomicAdd.

If C3/C6 is wrong and C1/C4 (intrinsic RMW race on chunks[]) is correct:
- SSIM remains in 0.69-0.81 cluster (cheap fence is structurally insufficient).
- AADF still reads `[0,0,0,0,1,1]`.
- [probe1-call] still shows the linear-drain pattern.
- This refutation rules out the entire C3/C6 fence-family in one shot;
  the bug is intrinsic to the chunks[]-RMW pattern (C1/C4).

### Implementation shape

1. Replace `render_context.command_encoder()` usage on wasm with a
   `render_device.create_command_encoder(...)` dedicated W3 encoder.
2. Encode the same n_rounds of {prepare, compute}.
3. Keep dispatch-2 iter-2-4's chunks-self-copy between rounds. (Not the
   subject of the probe; leave as-is since iter-2-4 produced 0.810 — the
   highest single SSIM achieved by any intervention. Removing it risks
   regressing baseline.)
4. After the loop, `render_queue.submit([w3_encoder.finish()])`.
5. Call `render_queue.on_submitted_work_done(|| {})` — forces Dawn to
   flush its internal queue.
6. Issue `slice.map_async(MapMode::Read, |_| {})` on a small COPY_DST |
   MAP_READ scratch buffer that received a `copy_buffer_to_buffer(chunks,
   fence_scratch, 4)` in the same encoder, to drive Dawn's submit-side
   flush.
7. Suppress the now-orphaned `end_of_encoder_noop_pipeline` resolve.
8. Native path unchanged.


### Iter-3-1 results

- Web SSIM 3 runs: **0.693519 / 0.693067 / 0.789544** — three for three FAILED. Same 0.69-0.79 cluster as baseline.
- Native: ran `--vox-horizon-native` → screenshot saved successfully; chunk@(242,31,219) AADF reads `[mx=31 px=31 my=10 py=31 mz=20 pz=9]` (multi-axis multi-round expansion — the canonical reference).
- Web AADF probe (early sampling, frame-N post-W3): chunk@(242,31,219) reads `word=0x02100000 chunk_aadf=[mx=0 px=0 my=0 py=0 mz=1 pz=1]` (run 3) and `word=0x00000000` all-zeros (run 2). Same as dispatch-2 baseline — only Z axis ever shows expansion.
- `[probe1-call]` pattern UNCHANGED: still linear drain `size0_ax0` 32768→28672→24576→...→4096 over 8 calls, then `size0_ax1`, then `size0_ax2`, etc. Web reaches size8 by call ~208-209.
- Run-3 reached size8 with 210 entries; runs 1,2 similar.

### Prediction vs actual (iter-3-1)

Prediction was: if C3/C6 (fence-family) is correct, SSIM stabilizes ≥0.91 on all 3 web runs AND [probe1-call] pattern shifts toward native (visit every (size, axis) once). Actual: SSIM in 0.69-0.79 cluster on all 3 runs; pattern unchanged. **C3/C6 (fence-family) is REFUTED.** Adding a dedicated W3 encoder + submit boundary + `on_submitted_work_done` + `map_async` fence does not fix the bug. The bug is intrinsic to the chunks[] RMW access pattern (C1/C4 family).

### Decision after iter-3-1

Per the synthesis recommendation: since C3/C6 (cheap fence) is REFUTED, move to iter 2 = chunks-history probe (synthesis §G item 2). This directly measures whether `compute_group_bounds`'s round-N read of `chunks[chunk_idx]` sees the round-(N-1) writes by other workgroups.


---

## Iteration 3-2 — Atomicise chunks reads/writes in bounds_calc.wgsl only

### Hypothesis statement (C1/C4 fix, scoped)

Per IMPLEMENTORS_SHARED.md Option C and synthesis §F+§G+§I-1: convert `chunks[]`
in `bounds_calc.wgsl` ONLY from `array<vec2<u32>>` to `array<atomic<u32>>` (twice
the elements: chunk_idx*2 = .x = AADF, chunk_idx*2+1 = .y = entity_y). Reads of
.x become `atomicLoad(&chunks[chunk_idx * 2u])`. Writes become
`atomicStore(&chunks[chunk_idx * 2u], cur_chunk)`. Other shaders that bind the
same buffer keep their existing `array<vec2<u32>>` declarations (separate bind
groups, separate WGSL views of the same underlying GPU buffer).

The hypothesis: atomic operations have proven cross-pass visibility on Dawn
(per Shape B / bound_queue_sizes which works), while non-atomic
storage-buffer reads do NOT (per dispatch-2 chunks-AADF probe finding).
Replacing the non-atomic load and the non-atomic store with atomicLoad +
atomicStore should make the cross-round chunks reads see the prior round's
writes.

We skip the chunks-history probe (synthesis §G item 2): dispatch-2's iter-2-1
AADF probe is already direct measurement of the bug — web reads `[0,0,0,0,1,1]`
versus native `[4,4,3,3,3,3]` — so additional instrumentation isn't needed
to know the bug is in chunks-RMW visibility. The probe would tell us WHAT
intermediate values web sees but we already KNOW from the AADF probe that
they're stale; the next move is to try the fix.

### Predict-the-outcome

If the C1/C4 atomic-load-fixes-visibility theory is correct:
- Web SSIM stabilises ≥0.91 on all 3 runs.
- Web AADF probe at chunk@(242,31,219) shifts toward native's
  `[mx=31 px=31 my=10 py=31 mz=20 pz=9]` (multi-axis multi-round expansion
  visible).
- [probe1-call] may also shift (the bound_queue_sizes drain via atomicAdd
  might also benefit if the underlying cross-pass visibility issue was on
  the same root cause).

If atomic-load doesn't help OR the bug is elsewhere:
- SSIM stays in 0.69-0.81 cluster.
- AADF still reads `[0,0,0,0,1,1]`.
- Then the next move would be atomicOr on the bits rather than
  atomicLoad+atomicStore RMW (a stronger fix, but requires algorithm change).


### Iter-3-2 results

- Web SSIM 3 runs: **0.792799 / 0.693159 / 0.692933** — same 0.69-0.79 cluster.
- Native AADF chunk@(242,31,219): `chunk_aadf=[mx=31 px=31 my=10 py=31 mz=20 pz=9]` — unchanged from baseline. No native regression.
- Web AADF chunk@(242,31,219): `chunk_aadf=[mx=0 px=0 my=0 py=0 mz=1 pz=1]` (or all-zeros depending on probe sampling timing) — UNCHANGED from dispatch-2 baseline.

### Prediction vs actual (iter-3-2)

Prediction: if atomic-load-fixes-visibility theory is correct, web SSIM ≥0.91 and AADF shows multi-axis multi-round values. Actual: SSIM in broken cluster; AADF unchanged. **The atomic-primitive fix on chunks[] DID NOT restore cross-pass visibility.** This is a surprising and significant finding — it implies that on Dawn/WebGPU, atomic operations on a 16 MiB storage buffer with many-workgroup writers do NOT have cross-pass cross-workgroup visibility the same way atomicAdd on the 384 B `bound_queue_sizes` buffer does. Either buffer size or writer-count matters for Dawn's atomic-coherence behavior.

---

## Iteration 3-3 — atomicAdd of delta (eliminate RMW write hazard, keep RMW read)

### Hypothesis statement

In iter-3-2 the `atomicStore` of `cur_chunk` replaced the prior non-atomic write
but per-round writes still OVERWRITE the chunks slot (no monotonic accumulation
across rounds). Per the synthesis Classification 4: each round reads stale 0,
modifies, writes — losing prior rounds' writes. Replacing the
`atomicStore(cur_chunk)` write with `atomicAdd(delta)` where
`delta = cur_chunk - cur_chunk_copy` makes the write monotonic-additive:
even if the read returns stale 0, the WRITE accumulates onto whatever is
already in the chunks slot from prior rounds. Subsequent rounds may still
read stale, but at least the accumulated chunks state at the FINAL frame
read by the renderer should be the OR/sum of all rounds' deltas.

### Predicted outcome

If atomicAdd-of-delta restores accumulation: AADF probe shows multi-axis expansion across rounds (`[1,1,1,1,1,1]` at minimum, with some axes >1). SSIM may move toward PASS.

If atomicAdd-of-delta is also ineffective (same Dawn cross-pass visibility issue applies to atomicAdd writes on chunks): SSIM stays in broken cluster, AADF still shows only last round's contribution.

### Results

- Web SSIM 3 runs: **0.796067 / 0.693492 / 0.693133** — same 0.69-0.79 cluster.
- Web AADF chunk@(242,31,219): unchanged from iter-3-2 — `chunk_aadf=[mx=0 px=0 my=0 py=0 mz=1 pz=1]`.

### Prediction vs actual (iter-3-3)

Prediction was: if atomic-add-of-delta works, AADF shows multi-axis values. Actual: AADF unchanged. **atomicAdd writes on chunks[] also do not propagate cross-pass on Dawn for this access pattern.** The chunks-state at the renderer-read frame ONLY contains the LAST round's delta — even though atomicAdd should have ACCUMULATED across rounds. The Dawn cross-pass visibility issue applies UNIFORMLY to all atomic ops on chunks (atomicLoad, atomicStore, atomicAdd).

---

## STATUS: BAILED (dispatch 3) — iteration 3 exhausted

### Hypotheses tried in dispatch-3

1. **iter-3-1 (Probe 1 — cheap fence probe).** Dedicated W3 encoder + separate `render_queue.submit` + `on_submitted_work_done` callback + `map_async` on fence-scratch + `device.poll(Poll)`. Per synthesis §G item 1, this discriminates C3/C6 (fence-family) from C1/C4 (intrinsic RMW). Web SSIM 0.694/0.693/0.790. AADF `[0,0,0,0,1,1]`. Pattern unchanged. **REFUTED C3/C6.** Cross-encoder/cross-submit fence does not fix the bug.

2. **iter-3-2 (atomicise chunks reads + writes in bounds_calc.wgsl only).** Per synthesis Option C / §F: `array<atomic<u32>>` view of the same buffer (paired indexing: chunk_idx*2 = .x, *2+1 = .y). atomicLoad + atomicStore everywhere chunks is touched in bounds_calc.wgsl. Other shaders keep `array<vec2<u32>>` views (separate bindings of same buffer). Web SSIM 0.793/0.693/0.693. AADF `[0,0,0,0,1,1]`. **REFUTED the narrow atomicLoad+atomicStore hypothesis** — atomic primitives on chunks[] do NOT restore cross-pass visibility on Dawn for this access pattern. This was the synthesis's TOP-RECOMMENDED fix shape and it did not work.

3. **iter-3-3 (atomicAdd-of-delta on chunks).** Replaces atomicStore-of-cur_chunk with atomicAdd of (cur_chunk - cur_chunk_copy) delta. Even if reads remain stale, writes accumulate monotonically. Web SSIM 0.796/0.693/0.693. AADF unchanged. **REFUTED atomicAdd-of-delta hypothesis** — atomicAdd writes on chunks[] also do not propagate cross-pass on Dawn. The chunks-state at the renderer-read frame ONLY shows the LAST round's contribution despite atomicAdd's monotonic accumulation semantics. This is the strongest evidence yet that Dawn's atomic-coherence for chunks-shaped (16 MiB, many-workgroup-writers) buffers is fundamentally broken in this version of Chrome's WebGPU implementation.

### Critical finding for next dispatch (REFRAMES the bug class)

**The synthesis's TOP-RANKED CLASSIFICATION (C1/C4 — chunks[]-RMW race fixable by atomicising) is REFUTED by iter-3-2 and iter-3-3.** Atomic operations (atomicLoad, atomicStore, atomicAdd) on chunks[] do NOT have cross-pass visibility on Dawn/WebGPU even when the underlying buffer is the same and only the WGSL declaration changes. The Shape-B success on `bound_queue_sizes` (384 B, few writers) does NOT generalize to `chunks` (16 MiB, 4096 × 64 = 262144 writers per round).

The dispatch-3 fence probe also refuted C3/C6 — cross-submit-boundary explicit fence does not help.

This leaves Classification 1 (cross-workgroup cross-pass NON-atomic storage visibility race) and Classification 5 (renderer/upload-path bug) as the surviving hypotheses, BUT in a STRONGER form than the synthesis originally framed: even ATOMIC primitives do not bypass the underlying visibility broken-ness for chunks-shaped buffers.

### Surviving hypotheses (post-dispatch-3)

1. **C1 (strengthened): Dawn's cross-pass cross-workgroup storage-visibility for buffers in the 1-16 MiB+ range with many writers is broken at a LOWER LEVEL than the WGSL atomic primitive can address.** This is consistent with both the non-atomic AND atomic chunks failures. Fix shape: algorithmic restructure (Shape G — queue rebuild from masks, eliminating the cross-pass chunks dependency) OR per-frame readback + CPU stitching (Shape C — already ruled out by dispatch-1 as structurally a non-starter on wasm).

2. **C5 (renderer-side or upload-path bug, AADF probe partial-state misleading).** The web AADF probe reads `[0,0,0,0,1,1]` which superficially "looks like W3 ran one round" — but if the chunks buffer the RENDERER reads is on a DIFFERENT page allocation from the chunks buffer W3 writes, OR if Dawn's bind-group caching captured a different buffer handle, we'd see this same symptom for a completely different reason. This was partially tested in iter-2-3 (read_write flip on renderer's chunks binding → regressed) but not via direct bind-group-cache investigation.

3. **NEW C7 (Dawn buffer-size-tier coherence threshold).** The fact that `bound_queue_sizes` (384 B) works while `chunks` (16 MiB) doesn't, even with identical atomic primitives, hints at a Dawn allocator tier boundary. Buffers above some size threshold may be allocated from a different VkBuffer pool with different VkMemoryPropertyFlags (e.g., HOST_COHERENT vs DEVICE_LOCAL-only). Testing shape: shrink the world to <1 MiB chunks total via a small test scene, see if the broken pattern persists. If shrinking fixes it, the bug is allocation-tier dependent.

### Recommended directions for next dispatch (4th)

In order of cost-effectiveness:

**A. Shape G — Algorithmic restructure (eliminate cross-pass chunks dependency).**
Instead of `compute_group_bounds` reading `chunks[chunk_idx]` to compute the new state, structure the algorithm so each round's chunks-WRITE is to a DIFFERENT BUFFER from the reads. E.g.:
- Round N writes per-thread delta entries into a sidecar `chunks_delta_ring: array<vec2<u32>>` (size: max_dispatched_chunks * n_bounds_rounds).
- Between rounds, a SCAN/reduce kernel coalesces the delta ring back into chunks via atomicAdd (cross-pass, but reducer is single-thread per chunk).
- Renderer reads chunks normally.
- This avoids the cross-pass cross-workgroup READ-MODIFY-WRITE pattern that Dawn doesn't handle.
- Cost: ~1 day implementation. Risk: algorithm correctness preservation.

**B. Bind-group rebuild every frame (probe C5).**
Force the renderer's `naadf_world_bind_group` to be rebuilt every frame from `world_gpu.chunks_buffer` (rather than using a cached bind group). If web SSIM stabilizes ≥0.91 after this, C5 is confirmed. Cost: 1-line edit in prepare.

**C. Shrink the world (probe C7).**
Try a 4x4x4 chunk world (256 B chunks) vs the current 256x32x256 (16 MiB). If small-world web SSIM passes consistently, the bug is Dawn allocator-tier dependent. Cost: 1-line config change + new test asset.

**D. Test isolation on a different machine / Chrome version.**
Pre-condition for the above: confirm the bug reproduces on a clean Chrome install (eliminating user-specific GPU driver / shader cache issues). The user has likely already verified this; if not, worth doing.

**E. Option D from dispatch-2: CPU fallback on wasm.**
Set `gpu_construction_enabled = false` on wasm. Loses GPU-acceleration but provides deterministic correctness. The horizon parity SSIM gate would PASS (CPU-built chunks state would be identical to native's CPU-built state — they share the same `aadf::construct::construct` codepath). The bevy-naadf project rule of "faithful port" allows wasm-specific divergences with explicit user approval; per the dispatch-2 recommendation this is the known-good escape hatch.

### Hard constraints encountered
Same as prior dispatches (no SSIM floor reduction, no MAX_RAY_STEPS bump, etc.).

### State of the worktree at bail time

Reverted to dispatch-2 iter-2-4 state (chunks non-atomic in WGSL; chunks-self-copy between rounds on wasm via Bevy's main encoder). The dispatch-3 iter-3-2 atomic chunks experiment and the iter-3-3 atomicAdd-of-delta experiment have been REVERTED. The iter-3-1 dedicated W3 encoder + fence experiment has been REVERTED.

The orchestrator inheriting state SHOULD restart from the same baseline as dispatch-3 (= dispatch-2 iter-2-4 = 0.69-0.81 statistical cluster with occasional lucky PASS).

Modified files at bail time (vs dispatch-2 iter-2-4 baseline): none of substance. Only logs + IMPLEMENTORS_SHARED.md and 09-synthesis-classifications.md docs grew.

### Logs / artefacts

Per-iteration logs in `target/diagnostics/consolidated/`:
- `iter-3-1/{web-run-{1,2,3,2-full,3-full}.log, native-run-1.log, native-build.log, web-build.log}` — iter-3-1 fence probe.
- `iter-3-2/{web-run-{1,2,3}.log, native-run-1.log, web-build.log}` — iter-3-2 atomicLoad+atomicStore.
- `iter-3-3/{web-run-{1,2,3}.log, web-build.log}` — iter-3-3 atomicAdd-of-delta.
- `iter-3-bail-baseline/web-run-1.log` — post-revert sanity check (0.795 in expected cluster).

Native AADF reference (`target/e2e-screenshots/vox_horizon_native.png` + `vox_horizon_native.aadf-probe.log`) and web visual (`vox_horizon_web.png`) preserved for comparison.

