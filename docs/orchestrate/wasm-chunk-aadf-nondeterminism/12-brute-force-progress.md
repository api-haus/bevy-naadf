# Brute-force progress notebook — wasm-chunk-aadf-determinism

PRIVATE; orchestrator does not read this. Append per attempt.

## Iteration 1 — HM/HN (host-side write_buffer fence between per-round submits)
REFUTED. SSIM=0.693450. Patterns unchanged.

## Iteration 2 — HP (chunks_mirror RO + chunks RW + copy between rounds)
REFUTED. SSIM=0.693512. Even structural read/write buffer separation
fails — chunks still reads as `[0,0,0,0,0,0]` at frame 60.

## Iteration 3 — HR (HP + atomicStore on chunks WRITES)
REFUTED across 3 runs. SSIM 0.789493 / 0.693407 / 0.693879.
First run slightly elevated (~0.79 intermediate band) but
not 3/3 PASS. ratio=4.936% / 4.147% / 4.147%.

## Pattern across iter-1..3 + all prior dispatches' 9 interventions

Every intervention falls into the 0.69-0.81 statistical cluster with
occasional 0.81-0.93 PASS lucky outliers. The 12 interventions (3 prior
+ 3 in this brute-force) have collectively tested:
- Per-round encoder + per-round submit
- One-encoder per frame (mirror native)
- Native code path on wasm (one encoder, indirect dispatch)
- End-of-encoder noop reading bound_queue_sizes
- End-of-encoder noop reading chunks[0]
- copy_buffer_to_buffer(chunks, scratch, 4) between rounds
- copy_buffer_to_buffer(chunks, scratch, 16 MiB) both directions
- Dedicated W3 encoder + on_submitted_work_done + map_async fence
- atomicLoad+atomicStore on chunks (W3 only)
- atomicAdd-of-delta on chunks
- Read-only chunks_mirror refreshed via copy + non-atomic write
- Read-only chunks_mirror refreshed via copy + atomicStore write
- Host-side write_buffer fence between rounds
- n_bounds_rounds raised to 40
- read_write chunks binding on renderer (regressed)
- Raising max_group_bound_dispatch from 4096 to 32768 (regressed)

NONE produced 3/3 PASS. The bug is below the WGSL atomic primitive,
below cross-encoder fences, below TRANSFER barriers, below atomic
storage decorations. The bug is in Dawn's storage-buffer cross-pass
write visibility AT A LEVEL THAT WGSL/WGPU CANNOT ADDRESS.

## ESCAPE: architectural change required

The right fix is to MOVE the W3 regime-2 AADF computation OFF the
per-frame GPU compute path on wasm and onto the CPU at startup.

### Why escape (not iterate further within bounded scope)
1. 12+ bounded interventions all refuted in the same statistical
   cluster. Probability of the 13th bounded intervention being the
   fix is vanishingly low.
2. The CPU-vs-GPU parity diagnostic at doc 10 ALREADY DEMONSTRATES
   that the CPU oracle (`aadf::bounds::compute_aadf_layer` with OOB-
   empty semantics — already implemented inline at mod.rs:4226-4340)
   produces the CORRECT chunks AADF state given the GPU's chunk
   classification.
3. The bug is empirically below the abstraction layer wgpu/WGSL
   exposes. We cannot fix it from within those abstractions; we can
   only sidestep it by doing the computation elsewhere.
4. The user-pinned SSIM floor (0.91) is achievable trivially by the
   CPU fallback; the deliverable here is correctness, not GPU
   acceleration.

### Why this WOULDN'T pass the brute-force budget
Even using my last 2 hypothesis slots, the realistic options are:
- HK (CPU fallback) — implementation IS architectural change.
- HQ (remove the cur_chunk_copy != cur_chunk guard) — diagnostic,
  unlikely to fix.
- HS (per-axis ordering randomization) — speculative + risky.
- HT (subgroup-size hints + workgroup_size adjustments) — possibly
  hits driver-side behaviour but speculative.

None of these are likely to produce 3/3 PASS given the empirical
trajectory of 12 prior refutations. The architectural escape clause
is explicitly there for this case.

## Architectural escape report — design sketch

### Scope estimate
- New file: `crates/bevy_naadf/src/render/construction/aadf_cpu_fallback.rs`
  (~200 LOC). Implements a wasm-only system that:
  1. Gates on `gpu.gpu_producer_has_run && !gpu.aadf_cpu_fallback_done`.
  2. Issues a `copy_buffer_to_buffer(chunks_buffer, staging_buffer)` +
     `map_async + AtomicBool` callback to read chunks back.
  3. On callback fire: decode chunks classification bits per chunk.
     Build is_empty closure. Run `compute_aadf_layer` with OOB-empty
     semantic (already implemented inline in `aadf_cpu_gpu_parity`).
     Pack AADF bits back into chunks words.
  4. `queue.write_buffer(chunks_buffer, 0, packed_chunks)` to upload
     the fully expanded AADF chunks back to GPU.
  5. Set `gpu.aadf_cpu_fallback_done = true`. Rebuild bind groups
     (force `world_gpu.bind_group = None` + render-system re-flow).

- Edit `mod.rs::ConstructionGpu`: add `aadf_cpu_fallback_done: bool`.

- Edit `bounds_calc.rs::naadf_bounds_compute_node`: on wasm, if
  `gpu.aadf_cpu_fallback_done`, early-return. The W3 GPU dispatch
  becomes a no-op on wasm post-fallback.

- Edit `mod.rs::ConstructionPlugin::build`: register the new system
  in ExtractSchedule on wasm only.

- Native: unchanged (the new system is `#[cfg(target_arch = "wasm32")]`-
  gated, so it doesn't fire).

### Files + line ranges
- `crates/bevy_naadf/src/render/construction/aadf_cpu_fallback.rs` (new)
- `crates/bevy_naadf/src/render/construction/mod.rs` (add field
  ~line 200; register system in plugin ~line 4680)
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs` (early-
  return gate ~line 500)

Effort: ~3-5 hours implementation; clean test path (the diagnostic
already proves the CPU oracle is correct).

### Why tactical fix is insufficient
12 interventions all in the same cluster — see "Pattern" above. The
underlying bug is in Dawn/Chrome's storage-buffer cross-pass visibility
behaviour for the chunks-RMW access pattern, at a level below WGSL
atomics, below intra-encoder TRANSFER barriers, below cross-encoder
fences. WebGPU spec doesn't expose tools to address this.

### Why this is escape (not in-scope iteration)
The brief defines bounded scope as: `bounds_calc.rs`, `bounds_calc.wgsl`,
`mod.rs`, `gpu_types.rs`, `bounds_calc/tests.rs`. The CPU-fallback fix
adds a new file `aadf_cpu_fallback.rs`. Even if implemented inline in
`mod.rs`, the fix conceptually moves W3 OFF the GPU pipeline — a
significant architectural shift.

### Recommended next dispatch
"Switch to distributed mode (delegate-architect → impl) scoped to
implementing CPU fallback for W3 regime-2 AADF on wasm. The CPU oracle
is already validated by the cpu-gpu-parity diagnostic. Implementation
should follow the documented Option D from dispatch-2's bail report
and dispatch-3's Option D recommendation."

Writing the escape report to 12-brute-force-summary.md now.
