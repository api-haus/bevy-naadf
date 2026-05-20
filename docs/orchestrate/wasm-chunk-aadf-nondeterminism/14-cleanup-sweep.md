# Tactical cleanup + Phase-2 SSIM sweep

## Status
PASS — all 4 items completed.

## Predict-the-outcome (recorded BEFORE the sweep)

Expect cluster 0.91-0.94, single-modal, 10/10 PASS. Rationale: doc 13 reported
5/5 PASS with the same minimal-fix set (config + iter-2 + iter-3-reverted),
with cluster 0.910-0.937. The two marginal runs (0.911, 0.911) sat within
0.002 of the floor — concerning, but the underlying mechanism (n=1 +
chunks_mirror per-encoder copy) is stable on the per-frame submission
boundary. The 0.91-0.94 width is most likely the trajectory-selector
bimodality flagged in doc-13's side-note 4 — over 10 runs we should see the
same dual-cluster shape (most runs in 0.92-0.94 band, occasional dips to
0.91), but the floor should hold because the chunks_mirror copy makes the
"unlucky" cluster less catastrophic than the funnel-data's 0.69/0.79 modes.
No items 1-3 modifications above touch any code in the chunks-write or
ray-march path, so they shouldn't affect SSIM at all.

If a run dips to 0.85-0.89 → recommend (not apply) iter-3 restoration.
If a run dips below 0.85 → the bimodal selector is regressing despite the
fix; iter-3 restoration applied.

### Post-sweep comparison
Prediction **CONFIRMED.** 10/10 PASS, cluster 0.911668-0.935101, median
0.930003, exactly the shape predicted — two marginal runs (1 + 3) hovering
just above the 0.91 floor, eight comfortable runs in the 0.925-0.935 band.
This matches doc 13's Phase 2 cluster (0.910951-0.937452 over 5 runs) and is
slightly tighter than the brute-force agent's reported 0.926-0.933 (3 runs).
No regressions from the cleanup edits (items 1-3) — they are pure
dead-code + docblock + buffer-size hygiene with no functional effects on
the chunks-write or ray-march path, as expected.

## Item 1 — dead Locals removed

| Local removed | File:line (pre-edit) | Verification |
|---|---|---|
| `chunks_scratch: Local<Option<Buffer>>` | `crates/bevy_naadf/src/render/construction/bounds_calc.rs:469` | Declared but only referenced via `let _ = chunks_scratch` at line 603 to suppress unused warning. Truly dead post-minimal-fix. |
| `chunks_scratch_for_fence: Local<Option<Buffer>>` | `crates/bevy_naadf/src/render/construction/bounds_calc.rs:474` | Declared for the iter-1 (HM/HN) host-side `queue.write_buffer` fence experiment. Only referenced via `let _ = chunks_scratch_for_fence` at line 604. Dead since iter-1 was abandoned. |

Both `let _ = ...` stubs at the old line 603-604 also deleted. Cleanup
compiled clean (`cargo check -p bevy-naadf --bin e2e_render` finished
without errors — see `target/diagnostics/cleanup-sweep/00a-check-after-item1.log`).
The cfg-attr `allow(unused_variables)` annotations on the surrounding
`world_gpu` / `render_device` / `render_queue` parameters were left in place
— those are real wasm-vs-native cfg-gated parameters, not iter-N artifacts.

## Item 2 — probe ring downsize

- Old size: 2048 entries × 16 B = 32 KiB
- New size: **256 entries × 16 B = 4 KiB**
- Rationale: at `n_bounds_rounds = 1` (the post-fix wasm regime), the
  regime-2 loop fires ~8 prepare-calls/frame at the slowest queue level.
  The probe readback is triggered at `PROBE_TRIGGER_FRAMES = 30` frames
  post-cpu-mirror (`mod.rs:3912`), so the useful capture window covers
  ~30 × 8 = 240 entries. 256 keeps a power-of-two and a small headroom
  (~16 entries) over the trigger window. Going smaller (64) would only
  capture ~8 frames of startup and lose visibility into the late
  pre-trigger drain. Going larger (1024+) wastes 12+ KiB of GPU storage
  that's never read.
- Files changed:
  - `crates/bevy_naadf/src/render/construction/mod.rs:328` —
    `PREPARE_PROBE_HISTORY_ENTRIES: u32 = 2048` → `256`. Docblock rewritten
    with the post-fix rationale.
  - `crates/bevy_naadf/src/render/construction/mod.rs:148-154` — docblock on
    `prepare_probe_history` field redirected to the const's docblock (the
    "2048 × 4 u32" inline comment removed).
  - `crates/bevy_naadf/src/render/construction/mod.rs:2061-2067` — allocation
    site comment updated to reference the new size + const-driven sizing.
  - `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl:163-167` —
    capacity comment updated.
  - `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl:404-411` —
    `prepare_group_bounds` inline-comment updated to reference the dynamic
    `arrayLength(&prepare_probe_history)` capacity.
- The WGSL is dynamic-capacity (`arrayLength(&prepare_probe_history) / 4u`)
  so no shader-side hardcode needs to change. The `tests.rs:529` test
  buffer is hardcoded to 2048 × 16 = 32 KiB; left unchanged — tests
  over-allocating is harmless (WGSL `arrayLength` adapts), and aligning the
  test would couple it to a const that's wasm-only motivated.

## Item 3 — docblock update

- File:line of edit: `crates/bevy_naadf/src/render/construction/config.rs:217`
  (appended after the pre-existing perf-throttling docblock; pre-edit ended
  at line 217 with "~0.5 ms on modern iGPU.").
- Old text (verbatim, the pre-existing block):
  ```
  /// **Steady-state bail cost** at 4_096: 5 rounds/frame × 4_096 workgroups
  /// × 64 threads = 1.3 M bail-out threads/frame; ~0.5 ms on modern iGPU.
  #[cfg(target_arch = "wasm32")]
  pub const WASM_MAX_GROUP_BOUND_DISPATCH: u32 = 4096;
  ```
- New text (verbatim, with the appended post-fix block):
  ```
  /// **Steady-state bail cost** at 4_096: 5 rounds/frame × 4_096 workgroups
  /// × 64 threads = 1.3 M bail-out threads/frame; ~0.5 ms on modern iGPU.
  ///
  /// **2026-05-20 post-fix update (`a426441` + `960eeb2`).** The original
  /// docblock above framed this constant as a perf-throttling lever paired
  /// with the direct-dispatch workaround for the Dawn STORAGE→INDIRECT
  /// barrier bug. That framing was incomplete. The full load-bearing fix
  /// for the wasm chunk-AADF non-determinism (ray-termination truncation,
  /// SSIM 0.69-0.93 cluster collapse on web) is the combination of:
  ///
  /// 1. The `n_bounds_rounds = 1` wasm clamp in `From<&AppArgs>` below — one
  ///    compute pass per frame eliminates the intra-encoder cross-pass write-
  ///    visibility race that Dawn empirically cannot mediate for the
  ///    `compute_group_bounds` chunks-RMW pattern. See commit `a426441`.
  /// 2. The `chunks_mirror` per-encoder `copy_buffer_to_buffer(chunks,
  ///    chunks_mirror)` infrastructure in `naadf_bounds_compute_node` +
  ///    the chunks_mirror RO bind-group entry. The TRANSFER-stage barrier
  ///    from the copy provides the cross-frame visibility that the bare
  ///    end-of-encoder submit boundary alone does not reliably give.
  ///    Reverting only this (keeping `n_bounds_rounds = 1`) regressed 1/3
  ///    web runs to SSIM 0.84.
  /// 3. (Inert layer — reverted in `960eeb2`.) An iter-3 atomicStore-on-
  ///    chunks WGSL pattern that turned out to be unnecessary once 1+2 are
  ///    in place.
  ///
  /// Cleanup characterization (item 4 of the cleanup-sweep dispatch) confirmed
  /// the 1+2 minimal-fix set holds an SSIM ≥ 0.91 floor across a 10-run web
  /// sweep. See `docs/orchestrate/wasm-chunk-aadf-nondeterminism/13-minimal-fix-verify.md`
  /// and `14-cleanup-sweep.md` for the full diagnostic story. Lowering this
  /// const further (smaller dispatch) would slow wasm convergence past the
  /// SSIM gate's 10 s settle. Raising it (32_768) regressed SSIM to 0.69.
  /// Re-baseline this and the n=1 clamp together if a deeper fix for the
  /// underlying WebGPU regime-2 cross-pass write visibility lands.
  #[cfg(target_arch = "wasm32")]
  pub const WASM_MAX_GROUP_BOUND_DISPATCH: u32 = 4096;
  ```

## Item 4 — Phase-2 SSIM sweep (≥10 web runs)

| Run | SSIM | Pass (≥ 0.91)? | Log path |
|---|---|---|---|
| 1 | 0.913804 | yes (marginal, +0.004) | `target/diagnostics/cleanup-sweep/web-run-1.log` (sidecar: `target/e2e-screenshots/funnel/vox_horizon_web-20260520T090934-991.txt`) |
| 2 | 0.932269 | yes | `target/diagnostics/cleanup-sweep/web-run-2.log` |
| 3 | 0.911668 | yes (marginal, +0.002) | `target/diagnostics/cleanup-sweep/web-run-3.log` (sidecar: `target/e2e-screenshots/funnel/vox_horizon_web-20260520T091132-212.txt`) |
| 4 | 0.926516 | yes | `target/diagnostics/cleanup-sweep/web-run-4.log` |
| 5 | 0.935101 | yes | `target/diagnostics/cleanup-sweep/web-run-5.log` |
| 6 | 0.927738 | yes | `target/diagnostics/cleanup-sweep/web-run-6.log` |
| 7 | 0.934019 | yes | `target/diagnostics/cleanup-sweep/web-run-7.log` |
| 8 | 0.925943 | yes | `target/diagnostics/cleanup-sweep/web-run-8.log` |
| 9 | 0.932825 | yes | `target/diagnostics/cleanup-sweep/web-run-9.log` |
| 10 | 0.933162 | yes | `target/diagnostics/cleanup-sweep/web-run-10.log` |

### Sweep statistics
- Min SSIM: **0.911668** (run 3)
- Max SSIM: **0.935101** (run 5)
- Median SSIM: **0.930003** (avg of run-6 0.927738 + run-2 0.932269, the
  5th + 6th rank-order values)
- Pass rate (≥ 0.91): **10/10**
- Cluster shape: **bimodal-ish, but stably above the floor.** 2 of 10 runs
  (1, 3) cluster around 0.912 (the "marginal" attractor); 8 of 10 runs
  cluster around 0.925-0.935 (the "lucky" attractor). The gap between
  the two clusters is ~0.013 SSIM (from 0.913804 → 0.925943, no runs in
  between). This matches doc 13's earlier 5-run observation: the
  trajectory-selector is still bimodal, but the chunks_mirror infrastructure
  + n=1 clamp lifts the unlucky attractor from the funnel-data's 0.69/0.79
  catastrophic modes to a stable ~0.912 marginal-pass cluster.

### Decision
**All 10 PASS — no further action.** The minimal fix set (`n_bounds_rounds
= 1` + `chunks_mirror` per-encoder copy_buffer_to_buffer, per commits
`a426441` + `960eeb2`) holds the SSIM ≥ 0.91 floor over 10 runs. The 0.013
gap between the marginal-cluster runs (1, 3) and the rest of the sample
suggests there's still a latent trajectory-selector that occasionally lands
in a slower-converging attractor, but the chunks_mirror copy is bringing
even that "unlucky" state to within ~0.002-0.004 of the floor. Iter-3
restoration was **not** required by the decision rule and is **not**
applied.

If a future regression touches the bounds-calc/ray-march path and re-pushes
the marginal cluster below 0.91, the proper response is to:

1. First investigate whether the marginal cluster has moved (re-run 10x).
2. If yes, restore iter-3 (atomicStore on chunks writes — the lines reverted
   in `960eeb2`) as a safety-margin layer. The brute-force agent's
   side-note 3 flagged the mixed-view (atomic + non-atomic) bind pattern
   as latent fragility, but reverting it is a cheap-to-re-apply safety
   net if the floor needs widening.

## Item 5 (conditional) — iter-3 restoration

N/A — decision rule met by 10/10 PASS without restoration.

## Native sanity (≥2 runs)

| Run | PASS/SSIM | Log path |
|---|---|---|
| 1 | PASS — chunk@(242,31,219) AADF=[mx=31 px=31 my=10 py=31 mz=20 pz=9], cpu-gpu-parity 100.000% / interior 100.000% | `target/diagnostics/cleanup-sweep/native-run-1.log` |
| 2 | PASS — identical AADF + 100.000% parity | `target/diagnostics/cleanup-sweep/native-run-2.log` |

Native unchanged. The probe ring downsize (item 2) does affect native too
(the const is not cfg-gated), but the WGSL is dynamic-capacity and the
native readback already only emits as many `[probe1-call]` lines as fit;
the smaller buffer just means fewer late-startup calls land. Native AADF
match between the two runs is bit-identical, and 100% cpu-gpu-parity is
preserved.

## Side notes / observations / complaints (MANDATORY per CLAUDE.md)

### Code smells noticed

1. **Stale serve.mjs from a deleted worktree squatted on port 4173.** The
   first web-run-1 attempt failed with all-404s because a `node serve.mjs`
   process from `wasm-aadf-minimal-fix/e2e` (now deleted) was bound to
   port 4173 from a prior session. Playwright's `webServer` block uses
   `reuseExistingServer: !process.env.CI` so it happily picks up the stale
   server's 404 responses instead of failing fast or spawning fresh. This
   is a footgun for any sequence-of-worktrees workflow. **Suggestion:**
   either set `reuseExistingServer: false` in `playwright.config.ts`, or
   add a `pretest` script that kills any stale `serve.mjs` listeners on
   :4173 before the suite runs. (Cost: ~10 min of confused trace-zip
   inspection per dispatch that hits this.)

2. **The `tests.rs:529` test probe buffer is hardcoded to `2048 * 16`** —
   not coupled to `PREPARE_PROBE_HISTORY_BYTES`. I left it as-is for this
   dispatch (the test over-allocates, which is harmless), but a future
   `/refactor` pass should align tests with the production const so future
   downsizes don't drift apart.

3. **The `[probe1-call]` ring's "256 is enough for 30 frames" calculation
   assumes the worst-case slowest-queue level of 8 prepare-calls/frame.**
   That number came from doc 12's `[probe1-call]` web log analysis — and
   the brute-force agent's claim was "~8 calls/frame at the slowest level"
   without showing the rate at faster levels. If a future change makes the
   per-frame call rate higher (e.g., a queue level that fires 30+ times),
   256 entries would drain in ~9 frames and the trigger window would
   under-sample. Worth a follow-up audit (read `[probe1-call]` lines in a
   web log, count by frame ID) to confirm. For now, 256 is defensible.

4. **The `naadf_bounds_compute_node` function still has multiple layers
   of "iteration N intervention preserved" commentary** — even after
   item 1's dead-code removal. The `chunks_mirror_buf_opt` extraction +
   the initial-seed copy + the per-round copy infrastructure are all
   load-bearing per doc 13's Phase-1 refutation, so they stay, but the
   surrounding comments still narrate the brute-force iter history. A
   future `/refactor` pass should consolidate this into a single coherent
   docblock at the top of the function explaining the n=1 + chunks_mirror
   cross-frame propagation pattern. (Out of scope here — item 5 of doc
   13's side-notes.)

5. **The web `[cpu-gpu-parity]` ratio stayed at ~18% across all 10 web
   runs** (visible in the funnel sidecars). The fix passes SSIM because
   the camera-view ray chunks expand correctly, not because the chunks
   buffer is universally correct — confirming the brute-force agent's
   side-note 3 ("a future fix to bring web parity to 100% would still be
   meaningful"). This is outside this dispatch's scope but worth flagging
   as a known correctness gap.

### Confusions in the brief

1. **The brief said "ring drains in ~8 frames at n=1; 2048 is
   over-instrumented"** which is correct, but its suggested "downsize to
   64" would have under-captured the 30-frame trigger window. I chose 256
   instead, documented why. The brief's "judgment" escape clause
   anticipated this — it was the right call to leave the floor open.

2. **The brief's "Run a 10-15-run Phase-2 web SSIM sweep" framing
   compresses two separate things:** (a) does the minimal fix hold the
   0.91 floor across more samples? (b) is the cluster width stable or
   widening? Question (a) was answered cleanly (10/10 PASS). Question (b)
   is harder to answer from 10 runs — the 0.911668 / 0.913804 marginal
   cluster might be the bottom of a wider distribution that hasn't fully
   shown its tail. A 30-run sweep would give a more confident answer. For
   now I'm reporting 10/10 PASS at the cluster shape observed, but
   recommending that any orchestrator concerned about long-tail behavior
   run a 30-run sweep periodically (e.g., in CI's "extended" mode).

### Subjective reactions

The cleanup edits felt low-risk and well-scoped — items 1-3 are textbook
post-fix hygiene. The 10-run sweep was the load-bearing item, and it
behaved as predicted. The marginal-cluster runs (0.911668 / 0.913804) are
concerning psychologically (so close to the floor!) but the gap to the
"lucky" cluster (0.013 SSIM = a meaningfully separated mode) suggests the
remaining variance is in the trajectory-selector logic, not in noise that
could drift the floor. If anything, item 4's result strengthens confidence
in the minimal-fix story: even the "unlucky" attractor reliably lifts to
≥ 0.91 with the chunks_mirror infrastructure in place.

The `node serve.mjs` squat-on-port issue cost ~15 minutes of trace-zip
forensics before I realized the URL was 404'ing at the server, not in the
wasm app. That was a useful diagnostic detour (confirmed the static-server
contract + the test-fixtures URL prefix) but it's a recurring footgun.

### What I wish I'd had context for

The brief's "(LFS phantom on `oasis.cvox`) DO NOT modify it" warning was
appreciated, but I'd have benefited from the orchestrator stating up front
that the worktree was branched from `960eeb2` with the LFS quirk already
in `git status` from-creation, not introduced mid-session. (`git status`
in my first command showed `~ Modified: 1 files crates/bevy_naadf/assets/test/oasis.cvox`
which is exactly the LFS phantom the brief warned about — confirmed
behaviour.) Saying "the worktree's `git status` will already show oasis.cvox
as modified — that's the LFS phantom, ignore it" would save the implementor
half a second of cognitive load.

### Anything else

The orchestrator's checkpoint-creator dispatch (separate from this) will
need to handle the LFS phantom dance — per the brief, the skip-worktree
trick. This dispatch deliberately did not run any git commands. The
deliverable above is the only doc to be committed (alongside the source
edits). All files modified by this dispatch:

- `crates/bevy_naadf/src/render/construction/bounds_calc.rs` (item 1)
- `crates/bevy_naadf/src/render/construction/mod.rs` (item 2)
- `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` (item 2)
- `crates/bevy_naadf/src/render/construction/config.rs` (item 3)
- `docs/orchestrate/wasm-chunk-aadf-nondeterminism/14-cleanup-sweep.md`
  (this deliverable)
