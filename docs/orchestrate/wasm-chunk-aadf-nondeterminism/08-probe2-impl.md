# Probe-2 implementation log — end-of-encoder no-op (M1 confirmation)

## Status

PASS — probe landed cleanly on both targets, no panics, no build errors, all
required runs completed. **M1 verdict: REFUTED.** The end-of-encoder no-op
did NOT shift web SSIM (cluster: 0.693, 0.695, 0.791 — pre-probe-2 baseline
was 0.78–0.81 per probe-1B + Shape-B-fix at 0.693) and did NOT change the
web `[probe1-call]` pattern (byte-identical to pre-probe-2 baseline:
`size0_ax0` drained linearly 32768→4096 over 8 calls, then `size0_ax1`,
etc.). Native unchanged (165 probe calls each run, no panic).

## Probe design (verified-implemented)

### WGSL no-op entry point

`crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl:410-441` (new entry
point added between `prepare_group_bounds` and `compute_group_bounds`).
Body verbatim:

```wgsl
@compute @workgroup_size(1, 1, 1)
fn end_of_encoder_noop() {
    let v = atomicLoad(&bound_queue_sizes[0u]);
    atomicStore(&bound_queue_sizes[0u], v);
}
```

The load-then-store-self pattern is semantically a no-op but irreducible:
the `atomicStore` consumes the `atomicLoad` result, so no compiler can elide
either operation. The entry point is **always declared** (not cfg-gated in
WGSL); only the Rust dispatch is wasm-gated.

### Rust pipeline creation

`crates/bevy_naadf/src/render/construction/bounds_calc.rs:281-321` — new
`queue_end_of_encoder_noop_pipeline` + `_with_handle` helpers. Pipeline
layout is `vec![world_layout, bounds_layout]` (same 2-layout shape as
`queue_compute_pipeline`); only `@group(1)` is actually accessed by the
entry-point body, but binding `@group(0)` keeps the layout symmetric with
the compute pipeline. Pipeline is queued ONCE in `ConstructionPipelines::from_world`
at `mod.rs:622-635`:

```rust
// 2026-05-20 probe-2 — `end_of_encoder_noop` pipeline (M1 probe).
// Always queued so the pipeline cache resolves on both targets, but
// only ever dispatched from the wasm-only branch in
// `naadf_bounds_compute_node`.
let bounds_calc_pipeline_end_of_encoder_noop =
    bounds_calc::queue_end_of_encoder_noop_pipeline(
        &asset_server,
        pipeline_cache,
        construction_bounds_world_layout.clone(),
        construction_bounds_layout.clone(),
    );
```

The new field `bounds_calc_pipeline_end_of_encoder_noop:
CachedComputePipelineId` was added to `ConstructionPipelines` at
`mod.rs:524-535` and populated in the `Self { … }` literal at `mod.rs:710`.

### Rust dispatch site

`crates/bevy_naadf/src/render/construction/bounds_calc.rs:541-571` — new
compute pass inside the wasm-only per-round encoder loop. Position: INSIDE
the same `round_encoder` as the existing prepare + compute passes, AFTER the
compute pass closes and BEFORE `render_queue.submit([round_encoder.finish()])`
runs. Dispatch is `(1, 1, 1)`. Verbatim (with the surrounding 5 lines of
context):

```rust
                pass.dispatch_workgroups(
                    construction_config.max_group_bound_dispatch.max(1),
                    1,
                    1,
                );
            }
            // 2026-05-20 probe-2 — end-of-encoder no-op dispatch (M1
            // confirmation probe). Runs as a third compute pass inside the
            // SAME `round_encoder`, AFTER `compute_group_bounds`'s writes
            // to `bound_queue_sizes` via `atomicAdd` and BEFORE the encoder
            // is finished + submitted below. The no-op binds `@group(1)`
            // and atomicLoad/atomicStore-s `bound_queue_sizes[0]` — that
            // makes it a "next user" of `bound_queue_sizes` to Dawn's
            // per-encoder PassResourceUsageTracker, which is expected to
            // insert a `vkCmdPipelineBarrier(SHADER_WRITE → SHADER_READ)`
            // between the compute pass and this no-op. That barrier is
            // an availability operation on `bound_queue_sizes`, which (per
            // `07-diagnosis-round2.md` Mechanism 1) should make compute's
            // last-writer-in-encoder atomicAdd writes propagate across
            // the subsequent `queue.submit(...)` boundary to the next
            // round's `prepare_group_bounds`. See WGSL `end_of_encoder_noop`
            // body for the load+store-self-irreducible no-op shape.
            {
                let mut pass = round_encoder.begin_compute_pass(
                    &bevy::render::render_resource::ComputePassDescriptor {
                        label: Some("naadf_bounds_calc_end_of_encoder_noop_pass_wasm"),
                        timestamp_writes: None,
                    },
                );
                pass.set_pipeline(end_of_encoder_noop_pipeline);
                pass.set_bind_group(0, bounds_world_bg, &[]);
                pass.set_bind_group(1, bounds_bg, &[]);
                pass.dispatch_workgroups(1, 1, 1);
            }
            // Submit this round in its own command buffer so the GPU
            // fences the atomic writes to `bound_queue_sizes[]`
            // …
            render_queue.submit([round_encoder.finish()]);
```

Per-round encoder structure is now: { prepare pass } { compute pass } **{ no-op pass }** finish + submit.

The no-op pipeline is resolved at `bounds_calc.rs:464-473` with `#[cfg(target_arch = "wasm32")]`:

```rust
// 2026-05-20 probe-2 — resolve the end-of-encoder no-op pipeline (wasm
// only). If the wasm-build hasn't yet resolved it, skip the node entirely
// (rather than dispatching prepare+compute without the probe — that would
// muddle the probe's signal).
#[cfg(target_arch = "wasm32")]
let Some(end_of_encoder_noop_pipeline) =
    pipeline_cache.get_compute_pipeline(
        construction_pipelines.bounds_calc_pipeline_end_of_encoder_noop,
    )
else {
    return;
};
```

### CFG gating

- WGSL: no cfg — entry point is always declared (Rust never dispatches it
  on native).
- Rust pipeline-cache field on `ConstructionPipelines`: always present.
- Rust pipeline queue call in `from_world`: always runs (cache resolves on
  both targets — confirmed by native runs which queue the pipeline but never
  dispatch it).
- Rust pipeline-handle resolution (`pipeline_cache.get_compute_pipeline(…)`):
  `#[cfg(target_arch = "wasm32")]` — native skips this guard.
- Rust dispatch site (3rd compute pass + bind-group + dispatch_workgroups):
  inside the existing `#[cfg(target_arch = "wasm32")] { … }` block at
  `bounds_calc.rs:482-561`.

## Source-code edits

| File | Lines changed | Purpose |
|---|---|---|
| `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` | +32 (new `end_of_encoder_noop` entry point + header doc) | WGSL entry point: 1-line body load+store-self irreducible no-op |
| `crates/bevy_naadf/src/render/construction/bounds_calc.rs` | +43 (`queue_end_of_encoder_noop_pipeline` + `_with_handle`) | New pipeline queueing helper (mirrors `queue_compute_pipeline`'s 2-layout shape) |
| `crates/bevy_naadf/src/render/construction/bounds_calc.rs` | +11 (`#[cfg(target_arch = "wasm32")]` pipeline-handle resolution) | Pull the no-op pipeline before entering the wasm loop |
| `crates/bevy_naadf/src/render/construction/bounds_calc.rs` | +30 (new compute pass after compute pass in wasm loop) | Dispatch the no-op `(1,1,1)` AFTER compute, BEFORE submit |
| `crates/bevy_naadf/src/render/construction/mod.rs` | +12 (`bounds_calc_pipeline_end_of_encoder_noop` field + doc) | New `CachedComputePipelineId` field on `ConstructionPipelines` |
| `crates/bevy_naadf/src/render/construction/mod.rs` | +11 (queue call in `from_world`) | Queue the no-op pipeline once at startup |
| `crates/bevy_naadf/src/render/construction/mod.rs` | +1 (struct-literal entry) | Wire the new field into the `Self { … }` literal |

Total: WGSL +32, Rust +108 (across 2 files), all additive. Probe-1B
instrumentation at `@group(3)` and the per-call `info!()` emission untouched.

### Diffs

WGSL — appended new `@compute` entry point between `prepare_group_bounds`'s
final `}` and `// ─── Entry point 3: compute_group_bounds — fx:118-193`
header:

```diff
+// ─── Entry point: end_of_encoder_noop — probe-2 (2026-05-20) ─────────────────
+//
+// M1-confirmation probe per `07-diagnosis-round2.md` Section I item 1. …
+// (full 28-line doc comment explaining purpose, expected Dawn barrier
+//  behavior, success/failure criteria)
+@compute @workgroup_size(1, 1, 1)
+fn end_of_encoder_noop() {
+    let v = atomicLoad(&bound_queue_sizes[0u]);
+    atomicStore(&bound_queue_sizes[0u], v);
+}
```

Rust `bounds_calc.rs` — appended `queue_end_of_encoder_noop_pipeline` +
`_with_handle` BEFORE the `// ─── Dispatch helpers ─` divider. Added the
`#[cfg(target_arch = "wasm32")]` `let Some(end_of_encoder_noop_pipeline) =
…` guard just before `let n_rounds = …`. Added the third compute-pass block
inside the existing wasm cfg branch, between the compute pass's closing `}`
and the `render_queue.submit([round_encoder.finish()])` call.

Rust `mod.rs` — three small additive edits to `ConstructionPipelines`'s
struct definition, `from_world` body, and struct literal — wiring the new
field through.

(`tests.rs` not modified — `W3Fixture` builds pipelines manually without
referencing `ConstructionPipelines`, so the new field is invisible to the
unit-test fixture.)

## Native runs (sanity)

| Run | Exit | `[probe1-call]` count | Panic-grep | Log |
|---|---|---|---|---|
| 1 | 0 | 165 (baseline = 165) | (no matches) | `target/diagnostics/probe2-noop/native-run-1.log` |
| 2 | 0 | 165 (baseline = 165) | (no matches) | `target/diagnostics/probe2-noop/native-run-2.log` |

**Native unchanged from baseline.** The probe-1B baseline at
`04-probe1-impl.md` reported 165 calls per native run, byte-for-byte
deterministic. Probe-2 reproduces exactly: 165 calls per run, no panic, no
GPU validation error, screenshot saved successfully each time. Confirms the
new pipeline field + `from_world` queue call do not regress native (which
never dispatches the no-op).

## Web build

- Exit: 0 (6 benign pre-existing unused-variable warnings, identical to
  pre-probe-2 baseline).
- Wall-time: ~9 s (cached).
- Log: `target/diagnostics/probe2-noop/web-build.log`.
- Fresh wasm: `crates/bevy_naadf/dist/bevy-naadf-ca3c329dae0a43c8_bg.wasm`,
  size 114691732 bytes, mtime 2026-05-20 04:23:33 (verified by `stat`).
- Trunk applied new distribution successfully.

## Web runs

| Run | Exit | SSIM | `[probe1-call]` first 10 pattern | Log | Artefacts |
|---|---|---|---|---|---|
| 1 | 1 | 0.693272 | `size0_ax0` drain 32768→4096 (8 calls) then `size0_ax1` 32768→28672 — IDENTICAL to pre-probe-2 baseline | `target/diagnostics/probe2-noop/web-run-1.log` | `target/diagnostics/probe2-noop/web-run-1-artefacts.txt` |
| 2 | 1 | 0.694839 | same as run 1 (byte-identical first-10 lines) | `target/diagnostics/probe2-noop/web-run-2.log` | `target/diagnostics/probe2-noop/web-run-2-artefacts.txt` |
| 3 | 1 | 0.791327 | same as run 1 (byte-identical first-10 lines) | `target/diagnostics/probe2-noop/web-run-3.log` | `target/diagnostics/probe2-noop/web-run-3-artefacts.txt` |

The "Exit 1" reflects the SSIM gate's `expect(parity).toBe(0)` failing — the
gate ran to completion, captured the screenshots, computed SSIM, and emitted
the full `[probe1-call]` drain. No panic, no `DeviceLost`, no
`RuntimeError`, no `Browser closed`, no `Test timeout` — all 215 probe
entries reached Playwright stdout, identical to probe-1B's 200–215 cluster.

First-10 `[probe1-call]` pattern for all 3 web runs (stripped of
%cINFO%c-style Bevy-log wrapping):

```
[probe1-call] call_idx=0 qi=size0_ax0 found_size=32768
[probe1-call] call_idx=1 qi=size0_ax0 found_size=28672
[probe1-call] call_idx=2 qi=size0_ax0 found_size=24576
[probe1-call] call_idx=3 qi=size0_ax0 found_size=20480
[probe1-call] call_idx=4 qi=size0_ax0 found_size=16384
[probe1-call] call_idx=5 qi=size0_ax0 found_size=12288
[probe1-call] call_idx=6 qi=size0_ax0 found_size=8192
[probe1-call] call_idx=7 qi=size0_ax0 found_size=4096
[probe1-call] call_idx=8 qi=size0_ax1 found_size=32768
[probe1-call] call_idx=9 qi=size0_ax1 found_size=28672
```

Compare to the pre-probe-2 baseline (probe-1B + Shape B fix, in
`06-fix-impl.md` line 222-233):

```
[probe1-call] call_idx=0  qi=size0_ax0 found_size=32768
[probe1-call] call_idx=1  qi=size0_ax0 found_size=28672
…
[probe1-call] call_idx=7  qi=size0_ax0 found_size=4096
[probe1-call] call_idx=8  qi=size0_ax1 found_size=32768
…
```

**Byte-identical.** The end-of-encoder no-op did NOT shift the pattern
toward native's "visit every (size, axis) once with found_size=32768"
shape. Web continues to drain `size0_ax0` linearly 4096-per-round across
8 calls, exactly as it did without the no-op probe.

## SSIM stability summary

- Min web SSIM: 0.693272 (run 1)
- Max web SSIM: 0.791327 (run 3)
- Spread: 0.098 — same order of magnitude as pre-probe-2 baseline cluster
  (0.78–0.81 in probe-1B; 0.693 single-run in 06-fix-impl).
- All ≥ 0.91? **NO.** Three out of three runs below the 0.91 floor.

## M1 verdict

- **REFUTED.**

The end-of-encoder no-op probe was constructed exactly per the architect's
spec in `07-diagnosis-round2.md` §I item 1: a `@compute @workgroup_size(1,
1, 1)` entry point reading + writing `bound_queue_sizes` via atomic ops,
dispatched as a third pass inside the same per-round encoder as compute,
positioned AFTER `compute_group_bounds` and BEFORE
`render_queue.submit(...)`. If Mechanism 1's hypothesis were correct —
that Dawn's per-encoder PassResourceUsageTracker would observe the no-op as
the "next user" of `bound_queue_sizes` and emit a
`vkCmdPipelineBarrier(SHADER_WRITE → SHADER_READ)` between compute's
writes and the no-op's read, thereby flushing compute's `atomicAdd` writes
into a state visible across the subsequent `queue.submit(...)` boundary —
then the cross-run SSIM should have jumped to ≥ 0.91 AND the web
`[probe1-call]` pattern should have shifted toward native's "every (size,
axis) visited once with found_size=32768" shape.

NEITHER condition is met: cross-run SSIM remains in 0.69–0.79 (below the
0.91 floor on all 3 runs), and the web `[probe1-call]` first-10 lines are
byte-identical to the pre-probe-2 baseline (linear drain of `size0_ax0`
4096-per-round across 8 calls, then `size0_ax1`). The intervention had no
measurable effect on either the SSIM number or the algorithm's observed
convergence pattern.

This refutes M1 as the load-bearing mechanism for web cross-pass atomic
invisibility. Possible explanations: (a) Dawn does NOT emit a
`vkCmdPipelineBarrier` between same-encoder compute dispatches that both
touch the same storage buffer with atomic ops (the
PassResourceUsageTracker behavior assumed by M1 is wrong); (b) Dawn DOES
emit the barrier but the barrier does NOT provide the cross-submit
availability operation M1 claims (i.e. Vulkan's
`vkCmdPipelineBarrier(SHADER_WRITE → SHADER_READ)` within an encoder is
insufficient for cross-submit visibility — the actual fix requires a
host-side fence, a `MAP_READ` operation, or a queue submission with a
semaphore signal); or (c) the cross-pass invisibility is upstream of any
encoder-internal barrier, e.g. in compute_group_bounds itself, or in
Tint's SPIR-V lowering of `array<atomic<u32>>` for this access pattern,
in a way that no encoder-level barrier insertion can fix. Mechanism 2
(Tint SPIR-V scope omission) gains plausibility under (c) but still
struggles to explain the prepare-visible-vs-compute-invisible asymmetry.

The architect's diagnose-first round 3 should now fire to design the next
probe / fix.

## Anomalies observed (raw, no diagnosis)

1. **Web SSIM run 3 (0.791) is markedly higher than runs 1 + 2 (0.693, 0.695).**
   The spread (0.098) is larger than the probe-1B baseline's 3-run spread
   (0.78–0.81 = 0.03). Whether the no-op marginally perturbs convergence
   pacing (likely — it adds a third compute pass per round) vs whether this
   is just baseline non-determinism in screenshot timing is unclear from 3
   runs alone.
2. **The `[probe1-call]` line count is steady at 215 across all 3 web runs**
   — exactly matching the post-Shape-B baseline in `06-fix-impl.md` and
   within the probe-1B range of 200–215. The no-op did not change how many
   prepare-calls fire by drain time.
3. **Native runs still report exactly 165 `[probe1-call]` lines per run** —
   probe-1B baseline. Confirms the no-op pipeline being queued (but never
   dispatched) on native does not perturb the native scheduler.
4. **No GPU validation errors, no `DeviceLost`, no `RuntimeError`, no
   `Browser closed`, no `Test timeout` on any of the 3 web runs.** The
   no-op pipeline binds, dispatches, and runs without issue on Dawn.
5. **The Playwright filter accepted the new entry point's dispatches
   silently** — no extraneous diagnostic lines appeared. The
   `prepare_probe_history` probe path was unaffected.

## Decisions & rejected alternatives

1. **Decision: WGSL body = `atomicStore(&bound_queue_sizes[0u],
   atomicLoad(&bound_queue_sizes[0u]))`** (load result feeding store). The
   brief listed this exact shape as the recommended defense against
   compiler elision. Reject alternative: a bare `atomicLoad` whose result
   is unused (any reasonable optimizer would elide); reject alternative:
   storing a constant (would not chain on the load, breaking irreducibility).
2. **Decision: pipeline layout = `vec![world_layout, bounds_layout]`**
   (mirrors `compute_group_bounds`'s 2-layout shape). Reject alternative:
   layout = `vec![bounds_layout]` only — would require the no-op to use
   `@group(0)` for its single binding, breaking symmetry with the existing
   compute pipeline. Reject alternative: a fresh single-binding layout for
   ONLY `bound_queue_sizes` — would force a separate bind group to be
   plumbed through `ConstructionBindGroups`, more invasive than the brief
   permits.
3. **Decision: dispatched in the SAME `round_encoder` as prepare + compute,
   as a third `begin_compute_pass` block** (separate pass, not a
   continuation of the existing compute pass). The brief explicitly noted
   both shapes (same pass OR new pass within same encoder) are acceptable;
   separate pass is cleaner (no need to also bind `@group(1)` to the
   compute pipeline's pass, no need to think about pipeline state between
   the compute dispatch and the no-op dispatch).
4. **Decision: pipeline-handle resolution is `#[cfg(target_arch =
   "wasm32")]`-only** — on native, the pipeline is queued (so the cache
   resolves it during normal `process_pipeline_queue_system` ticks) but
   never resolved into a `&ComputePipeline` reference, so the queueing
   overhead is minimal and the dispatch is unreachable. Reject alternative:
   skip queueing on native entirely — would have required a `#[cfg]` in
   `from_world` that complicates the `Self { … }` literal.
5. **Decision: ran all 3 web runs even after run-1 showed M1 was refuted
   (SSIM = 0.693, pattern unchanged).** The brief specifies probe-2 is an
   instrumentation probe, NOT a fix; the "stop-on-failure" rule applies to
   fix runs where the next run would risk hiding regressions. For a probe,
   the right move is to gather full cross-run data to confirm the refutation
   is robust across stochastic browser pacing — and indeed runs 2 + 3
   reproduced the same byte-identical `[probe1-call]` pattern, eliminating
   any "maybe it works sometimes" interpretation.

## Assumptions made

1. **`bound_queue_sizes[0]` is a safe load+store target.** Slot 0 is
   `(size=0, axis=0)` per the indexing convention `qi = BOUND_INFO_GROUPS +
   size * 3 + axis`. `BOUND_INFO_GROUPS = 0`. The no-op writes back the
   same value, so even if compute or prepare wrote slot 0 between this
   no-op and the next prepare, the write would be lossless. (Verified by
   reading the WGSL: the no-op stores `v` which IS `atomicLoad(slot 0)`,
   so any cross-pass write that landed during the no-op's load→store
   window would itself be overwritten by `v`. This is a load-store-self
   race in the worst case but not a correctness violation — the next
   prepare will re-read the slot.)
2. **The probe-1B `prepare_probe_history` writes on `@group(3)` are
   unaffected by the new pass.** The no-op only binds `@group(0)` +
   `@group(1)`, so probe-1B's write path is independent. Confirmed by
   the per-call lines reaching Playwright stdout identically to the
   pre-probe-2 baseline.
3. **Dawn's PassResourceUsageTracker per the architect's `07-diagnosis-round2.md`
   §D.1 should observe the no-op as the "next user" of `bound_queue_sizes`.**
   This is the architect's hypothesis; the probe is designed to confirm or
   refute it. The empirical outcome (no effect on SSIM or pattern)
   REFUTES this hypothesis as stated, OR refutes the chain of reasoning
   from "Dawn emits a `vkCmdPipelineBarrier`" to "compute's writes become
   cross-submit visible." Either way the corollary that "an end-of-encoder
   barrier would fix the bug" is empirically wrong.
4. **The pipeline-cache `get_compute_pipeline(…)` resolves the no-op
   pipeline before the first invocation of the regime-2 node.** Verified
   by the web runs' 215 `[probe1-call]` lines per run — if the no-op
   pipeline had not resolved, the wasm branch's `let Some(end_of_encoder_noop_pipeline)
   = …` guard would have early-returned, no dispatch would have run, and
   the per-call probe would have shown 0 calls. The presence of 215 calls
   per run confirms the regime-2 node IS running with the no-op dispatched.

## Artifacts on disk (absolute paths)

Logs:
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe2-noop/00-cargo-check.log`
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe2-noop/01-cargo-build-e2e.log`
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe2-noop/web-build.log`
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe2-noop/native-run-1.log` (165 `[probe1-call]` matches)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe2-noop/native-run-2.log` (165 matches)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe2-noop/web-run-1.log` (SSIM 0.693272, 215 matches)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe2-noop/web-run-2.log` (SSIM 0.694839, 215 matches)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe2-noop/web-run-3.log` (SSIM 0.791327, 215 matches)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe2-noop/web-run-1-head10.txt`
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe2-noop/web-run-2-head10.txt`
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe2-noop/web-run-3-head10.txt`
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe2-noop/web-run-1-artefacts.txt`
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe2-noop/web-run-2-artefacts.txt`
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/probe2-noop/web-run-3-artefacts.txt`

Screenshots (overwritten per-run, last write wins):
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/vox_horizon_native.png` (latest native; native unaffected by probe)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/vox_horizon_web.png` (latest web)

Fresh wasm:
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/crates/bevy_naadf/dist/bevy-naadf-ca3c329dae0a43c8_bg.wasm` (mtime 2026-05-20 04:23:33)

Source-code edits (this dispatch):
- `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` (+1 entry point + doc)
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs` (+pipeline helpers, +cfg-gated pipeline-handle resolution, +cfg-gated dispatch pass)
- `crates/bevy_naadf/src/render/construction/mod.rs` (+1 field on `ConstructionPipelines`, +1 queue call in `from_world`, +1 entry in struct literal)
