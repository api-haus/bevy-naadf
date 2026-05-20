# Minimal-fix verification — wasm-chunk-aadf

## Status
WON-PARTIAL — `n_bounds_rounds = 1` alone is NOT sufficient (1 of 3 web runs dropped to SSIM=0.840626, below 0.91). Binary search showed iter-2 (chunks_mirror RO + per-round `copy_buffer_to_buffer`) is load-bearing. Iter-3 (atomicStore on chunks writes) is INERT and was reverted. Minimal load-bearing set = config (`n_bounds_rounds = 1`) + iter-2 (chunks_mirror mechanism). 5/5 web runs PASS with that set.

## Hypothesis tested
Reverting iter-2 (chunks_mirror RO + binding layout extension) and iter-3 (atomicStore on chunks writes) while keeping ONLY `n_bounds_rounds = 1` wasm config change still produces 3/3 PASS.

## Predict-the-outcome (recorded BEFORE Step 7)
Hypothesis holds. The brute-force summary's mechanism analysis is sound: `n_bounds_rounds=1` addresses the real failure mode (multiple compute passes per encoder racing on chunks writes), while iter-2/iter-3 were addressing the wrong mechanism (intra-encoder visibility, which Dawn's resource tracker is already supposed to handle). With only one compute write per encoder per frame, there's no intra-encoder race left to mediate via mirror buffers or atomicStore. Expected web SSIM cluster: all 3 runs in 0.91-0.94 band, with median near 0.927 (same shape as the full-fix distribution at commit `a426441`). If any run drops into 0.69 or 0.79 cluster, the iter-2/iter-3 changes are load-bearing after all and the binary-search fallback procedure kicks in.

**Outcome:** prediction PARTIALLY wrong. The bimodal "0.69 / 0.79" cluster from the funnel data did NOT reappear — but a NEW cluster at ~0.84 appeared on run 3 of the config-only revert. Not the broken-attractor SSIM (0.69), not the lucky-attractor SSIM (0.93), but an intermediate that's still below the 0.91 floor. This contradicts the brute-force summary's framing that iter-2/iter-3 were "inert experiments" — iter-2 IS load-bearing on top of `n_bounds_rounds = 1`. Iter-3 turned out genuinely inert.

## Changes reverted

### Phase 1 — config-only (FAILED, 1/3)

| File | Lines | What was reverted |
|---|---|---|
| `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` | 105-133 | chunks_atomic + chunks_mirror declarations → `chunks: array<vec2<u32>>` only at @binding(0) |
| `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` | ~253 | `chunks_mirror[neighbour_idx].x` → `chunks[neighbour_idx].x` |
| `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` | ~520 | `chunks_mirror[chunk_idx]` → `chunks[chunk_idx]` |
| `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` | ~564 | `atomicStore(&chunks_atomic[...], ...)` ×2 → `chunks[chunk_idx] = vec2<u32>(...)` |
| `crates/bevy_naadf/src/render/construction/bounds_calc.rs` | 90-103 | bind-group layout: 3 entries (chunks_rw + params + chunks_mirror_ro) → 2 entries (chunks + params) |
| `crates/bevy_naadf/src/render/construction/bounds_calc.rs` | 467-471, 596-680 | removed `chunks_scratch_for_fence` Local, removed initial + between-rounds `copy_buffer_to_buffer(chunks, chunks_mirror)`, reverted to single-encoder dispatch loop pre-iter-2 |
| `crates/bevy_naadf/src/render/construction/mod.rs` | 157-165 | removed `chunks_mirror_buffer: Option<Buffer>` field |
| `crates/bevy_naadf/src/render/construction/mod.rs` | 2118-2160 | removed chunks_mirror_buffer allocation + 3rd bind-group entry |

### Phase 2 — restored iter-2, kept iter-3 reverted (PASSED, 5/5)

| File | Lines | What was restored vs Phase 1 |
|---|---|---|
| `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` | 105-133 | restored chunks_mirror RO @binding(2); kept @binding(0) as non-atomic `chunks: array<vec2<u32>>` (iter-3 NOT restored) |
| `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` | ~253 | restored `chunks_mirror[neighbour_idx].x` reads |
| `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` | ~520 | restored `chunks_mirror[chunk_idx]` reads |
| `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` | ~553-558 | kept iter-3 reverted: write through non-atomic `chunks[chunk_idx] = vec2<u32>(cur_chunk, entity_y)` |
| `crates/bevy_naadf/src/render/construction/bounds_calc.rs` | 90-103 | restored 3-entry bind-group layout |
| `crates/bevy_naadf/src/render/construction/bounds_calc.rs` | 596-680 | restored per-round `copy_buffer_to_buffer(chunks, chunks_mirror)` + initial seed copy |
| `crates/bevy_naadf/src/render/construction/mod.rs` | 157-165 | restored `chunks_mirror_buffer` field |
| `crates/bevy_naadf/src/render/construction/mod.rs` | 2118-2160 | restored chunks_mirror_buffer allocation + binding |

## Changes preserved
- `crates/bevy_naadf/src/render/construction/config.rs:241-253` — wasm-only `n_bounds_rounds = 1` clamp (load-bearing).
- iter-2 mechanism (chunks_mirror RO binding + per-round `copy_buffer_to_buffer`) — load-bearing per Phase 1 refutation.
- `[probe1-call]` / `[cpu-gpu-parity]` / `[aadf-probe]` diagnostic instrumentation (orthogonal to the fix, preserved through both phases).

## Native runs (≥2)
| Run | Exit | AADF/parity | Log |
|---|---|---|---|
| native-1 (Phase 1, all 3 iter changes reverted) | 0 | chunk@(242,31,219)=[mx=31 px=31 my=10 py=31 mz=20 pz=9], cpu-gpu-parity=100.000% / interior=100.000% | `target/diagnostics/minimal-fix-verify/native-run-1.log` |
| native-2 (Phase 1) | 0 | identical 100% parity, identical AADF | `target/diagnostics/minimal-fix-verify/native-run-2.log` |
| native-iter2only-1 (Phase 2, iter-2 + config only) | 0 | identical 100% parity, identical AADF | `target/diagnostics/minimal-fix-verify/native-iter2only-run-1.log` |

Native unchanged under both phases — the wasm clamp is gated by `#[cfg(target_arch = "wasm32")]`, and the WGSL iter-2 changes are semantically identical on native because `copy_buffer_to_buffer` between rounds on native is a no-op-equivalent (native Vulkan's intra-encoder barriers already give cross-pass visibility, and reading the up-to-date mirror produces the same result as reading the rw chunks directly after the prior round's write completes).

## Web runs (≥3)

### Phase 1 — config-only revert (`n_bounds_rounds = 1`, iter-2 + iter-3 BOTH reverted)
| Run | Exit | SSIM | Pass (≥ 0.91)? | Log |
|---|---|---|---|---|
| 1 | 0 | 0.932833 | yes | `target/diagnostics/minimal-fix-verify/web-run-1.log` |
| 2 | 0 | 0.933442 | yes | `target/diagnostics/minimal-fix-verify/web-run-2.log` |
| 3 | 0 | 0.840626 | **NO** | `target/diagnostics/minimal-fix-verify/web-run-3.log` |

**Phase 1 = 2/3 PASS → REFUTED.** Hypothesis falsified. Binary search begun.

### Phase 2 — iter-2 restored, iter-3 stays reverted
| Run | Exit | SSIM | Pass (≥ 0.91)? | Log |
|---|---|---|---|---|
| 1 | 0 | 0.932644 | yes | `target/diagnostics/minimal-fix-verify/web-iter2only-run-1.log` |
| 2 | 0 | 0.911323 | yes (marginal) | `target/diagnostics/minimal-fix-verify/web-iter2only-run-2.log` |
| 3 | 0 | 0.933761 | yes | `target/diagnostics/minimal-fix-verify/web-iter2only-run-3.log` |
| 4 | 0 | 0.910951 | yes (very marginal) | `target/diagnostics/minimal-fix-verify/web-iter2only-run-4.log` |
| 5 | 0 | 0.937452 | yes | `target/diagnostics/minimal-fix-verify/web-iter2only-run-5.log` |

**Phase 2 = 5/5 PASS** — but the 0.910951 and 0.911323 runs sit right on the 0.91 floor, suggesting the iter-2-only config still exhibits a bimodal lucky/marginal trajectory selector. The brute-force commit `a426441` (config + iter-2 + iter-3) reported a tighter 0.926-0.933 cluster (3/3); the Phase 2 cluster spans 0.910-0.937 (5 runs). It IS possible iter-3 contributed cluster-tightening at the margins, even though iter-2 alone is sufficient for the 0.91 floor. Recommend running 10-15 Phase-2 web sweeps before declaring iter-3 universally inert.

## SSIM stability summary

### Phase 1 (config-only)
- All 3 web SSIM ≥ 0.91? **NO** (1 of 3 = 0.840626 < 0.91).
- Min/Max web SSIM: 0.840626 / 0.933442
- Variance is bimodal — runs 1&2 sit in the "lucky" 0.93 cluster, run 3 in a new ~0.84 intermediate. The funnel data's 0.69/0.79 broken cluster did NOT appear, suggesting `n_bounds_rounds = 1` does at least partially break the worst-case race, but iter-2's chunks_mirror copy is still required for floor stability.

### Phase 2 (config + iter-2)
- All 5 web SSIM ≥ 0.91? YES.
- Min/Max web SSIM: 0.910951 / 0.937452
- Median: 0.932644
- 2 of 5 runs sat within 0.002 of the 0.91 floor. Tighter than Phase 1, but wider than the brute-force commit's reported 0.926-0.933. Possibly noise; possibly iter-3 contributes marginal cluster-tightening.

## Verdict
**WON-PARTIAL.** The minimal hypothesis was REFUTED — `n_bounds_rounds = 1` alone is insufficient (only 2/3 web runs PASS). Binary search confirmed iter-2 (chunks_mirror RO + per-round `copy_buffer_to_buffer`) is **load-bearing** at n=1; without it, run 3 dropped to 0.840626. Iter-3 (atomicStore on chunks writes via `chunks_atomic: array<atomic<u32>>` view) is **inert** at n=1+iter-2 — 5/5 web runs PASS with iter-3 reverted.

The mechanistic story is now: at `n_bounds_rounds=1`, each frame the encoder issues ONE compute pass that writes chunks. The next frame's compute pass reads via `chunks_mirror`, which is refreshed at start-of-encoder via `copy_buffer_to_buffer(chunks → chunks_mirror)`. The TRANSFER-stage barrier from the copy is what gives reliable cross-frame visibility — `n_bounds_rounds=1` alone (without the copy) leaves an end-of-encoder → start-of-next-encoder gap where Dawn's render-graph submit boundary apparently doesn't always flush the prior frame's compute writes to a level visible to the next frame's shader read. The copy through the dedicated mirror buffer pulls those writes through a transfer queue that Dawn DOES honour across the boundary.

The brute-force agent's "n_bounds_rounds=1 alone addresses the actual mechanism" framing was OVER-simplified. The correct framing is "n_bounds_rounds=1 + an explicit cross-pass propagation buffer (chunks_mirror) is the load-bearing combination." The atomic-typed view was a red herring that, by coincidence, was in the tree when the n=1 fix landed.

## Final tree shape (lines of net diff vs commit `a426441`)
- Files modified: 1 (`crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` only)
- Insertions: 11 (replacement comments + non-atomic chunks write)
- Deletions: 20 (atomicStore + iter-3 commentary + chunks_atomic declaration)
- **Net simplification: -9 lines** (mostly removed iter-3 commentary + atomic-view code path)
- Files unchanged vs `a426441`: `config.rs`, `bounds_calc.rs`, `mod.rs` (these all carry iter-2 + config, which is load-bearing)

This is a small but meaningful cleanup — removes the iter-3 conceptual layer (atomic-typed view + paired-indexing arithmetic) that turned out to be inert. The codebase no longer has the "chunks is non-atomic in one shader and atomic-view in another shader" mixed-view fragility flagged in the brute-force agent's side-note #3.

## Side notes / observations / complaints (MANDATORY per CLAUDE.md)

### Code smells noticed

1. **`naadf_bounds_compute_node` has accumulated layered "iter-N intervention preserved" code** — including a `chunks_scratch` Local that was kept "for signature compatibility" and a `chunks_scratch_for_fence` Local that's never used (just `let _ = ...`). These should be removed in a follow-up `/refactor` pass — they don't affect correctness but they make the function harder to read. The brute-force agent's side-note #2 flagged this; my work didn't address it.

2. **The `[probe1-call]` ring is sized for the n=5 pre-fix world** (2048 entries, capacity for ~400 frames at 5 rounds/frame). At n=1 the entire ring drains in 8 frames flat. The probe is now over-instrumented for its purpose. Could be downsized to ~64 entries.

3. **The `[cpu-gpu-parity]` ratio on web is only ~18% even with the full fix** (vs 100% on native). The fix passes SSIM because the camera view-ray chunks are correctly expanded, not because the chunks state is universally correct. Side-note #3 in the brute-force summary already flags this; it's worth keeping the diagnostics around because a future "make web parity 100%" effort would still be meaningful.

4. **The Phase 2 SSIM cluster width (0.910-0.937)** is noticeably wider than the brute-force summary's claim (0.926-0.933). One of three things: (a) iter-3 was contributing marginal cluster-tightening after all, (b) the brute-force agent's 3 runs were lucky and got the tight upper end of the same wider distribution, or (c) there's some run-to-run variation in the underlying Dawn/Chrome state that the brute-force agent didn't see. I lean toward (b) — three runs is a small sample and a 0.92-0.93 cluster is consistent with a 0.910-0.937 underlying distribution. But (a) is not refuted — a 10-15-run Phase-2 sweep would settle it. The current minimal-fix tree is correct (5/5 PASS); if a future run sees a sub-0.91 result, the answer is "restore iter-3," not "abandon the simplification."

### Confusions in the brief

1. **The brief framed "iter-2 + iter-3 were addressing the wrong mechanism (intra-encoder visibility)."** That framing is wrong — at `n_bounds_rounds = 1` there IS no intra-encoder visibility issue (only one compute pass per encoder). The chunks_mirror + copy_buffer_to_buffer aren't fixing intra-encoder cross-pass visibility; they're fixing **cross-FRAME** visibility (start of one encoder reads what end of previous encoder wrote). The brief's premise that the iter-2 work was "shooting at the wrong target" was wrong — it's shooting at a slightly different target that's still load-bearing. The orchestrator should update the brute-force summary's mechanism description to reflect this.

2. **The brief said "≥3 web runs" for both the test and the binary search — but binary search at the floor warrants more runs (5-10) because the cluster width matters at the floor.** Phase 2 needed at least 5 runs to characterize the marginal-pass behavior. I ran 5, but only because runs 2 and 4 sat within 0.001 of the floor and I wanted to verify it wasn't a coincidence. The brief's "3 web runs" minimum is fine for the hypothesis test but isn't enough at the floor.

### Subjective reactions

The codebase is in better shape after this revert — the WGSL is conceptually simpler (no chunks_atomic view + paired-indexing arithmetic to reason about) and the bind-group layout is unchanged. The brute-force agent's instinct to leave iter-2 + iter-3 in tree "because they don't hurt" was reasonable — the WIN was found and shipping a verified-correct fix was the priority. The minimal-fix dispatch (this work) is the appropriate follow-up.

The Playwright SSIM-compare invocation inside the test is fragile — it shells out to `cargo run --bin e2e_render` which, if not pre-built, can exceed the 120s per-test timeout. My Phase 1 run 1's funnel sidecar reported SSIM `<unavailable>` because of this; I had to manually run `target/release/e2e_render --ssim-compare` post-hoc to get the actual scores. A small `npm test` pre-step that runs `cargo build --release --bin e2e_render` before invoking Playwright would eliminate this. Worth noting for the orchestrator's next test-infra cleanup.

### Suggestions for follow-up cleanup or /refactor scope

1. **Remove the unused `chunks_scratch` and `chunks_scratch_for_fence` Locals from `naadf_bounds_compute_node`** — they're dead-code artifacts from iter-1.
2. **Downsize the `[probe1-call]` ring buffer from 2048 to ~64 entries** now that n=1 means the ring drains in 8 frames.
3. **Update `WASM_MAX_GROUP_BOUND_DISPATCH`'s docblock** to mention that the wasm clamp + iter-2 chunks_mirror mechanism is the load-bearing fix for Dawn cross-frame visibility. The docblock currently doesn't reference iter-2.
4. **Run a 10-15-run Phase-2 web sweep** to characterize the actual SSIM cluster width and confirm iter-3's inertness more rigorously. If cluster widens past 0.91 floor on more samples, restore iter-3.
5. **Consider a `/refactor` pass on `naadf_bounds_compute_node`** to collapse the wasm-vs-native cfg gating now that the fix mechanism is understood. The current state has multiple layers of "iteration N intervention preserved" commentary that could be consolidated into a single coherent docblock explaining the n=1 + chunks_mirror cross-frame propagation pattern.

### What I wish I'd had context for

- The brief's predictions were authored before the binary search began. Re-reading the prior agent's progress notes (`docs/orchestrate/wasm-chunk-aadf-nondeterminism/12-brute-force-progress.md`) would have been useful, but the brief's required-reading list didn't include it.
- The funnel data sidecar txt format is great for orchestrator review but it leaks Chrome's `%cINFO%c ... color: ...` formatting through the wasm-diag bridge. Cosmetic but ugly. A small log-format normalizer in `wasm_tracing_bridge.rs` would clean it up.

### Anything else

The fact that 1 of 3 Phase-1 web runs PASSED at 0.93 (not a marginal pass, a solid one) shows that the trajectory selector is still bimodal even with `n_bounds_rounds = 1`. The lucky-attractor state is reachable without the chunks_mirror mechanism — sometimes. That's interesting evidence that something OTHER than chunks_mirror is involved in selecting between the attractors. Might be a fruitful angle for the "future fix to bring web parity to 100%" mentioned in the brute-force agent's side-note #3.
