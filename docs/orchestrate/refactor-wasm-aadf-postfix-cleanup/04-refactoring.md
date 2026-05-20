# 04-refactoring — refactor-wasm-aadf-postfix-cleanup

Empty stub. The `refactor-implementer` agent writes the per-step
execution log here under the heading `## refactor-implementer log (<ISO date>)`.

## refactor-implementer log (2026-05-20)

### Overall status

**PASS — all 5 design steps applied; native + web e2e gates green (3/3 web SSIM PASS ≥ 0.91); cargo check workspace clean.**

Caveat 1: `cargo test -p bevy-naadf --lib` blocked by **pre-existing** baseline compile errors at `crates/bevy_naadf/src/render/construction/mod.rs:9725` and `:10831` (`dispatch_offset` field references vs struct definition). Verified by stashing my changes and re-running on baseline commit `1fdd256` — identical errors. Not introduced by this refactor; not in scope to fix.

Caveat 2: Web e2e gate required a workaround for a pre-existing race in
`e2e/playwright.config.ts` + `e2e/kill-stale-server.mjs` — the globalSetup
kills the webServer Playwright itself just started (Playwright's lifecycle
runs `webServer.command` before `globalSetup`, opposite of the
`15-playwright-stale-server-fix.md` design intent). Verified the failure
mode reproduces on baseline `1fdd256` source — pre-existing. Worked around
locally by running a watchdog (`while true; do node serve.mjs; sleep 0.5; done &`)
that respawns the server within ~0.5s of the kill. 3/3 web runs PASS with
that watchdog. See § Side notes for a full smell-flag.

### Step-by-step

#### Step 1 — Couple `tests.rs` probe-buffer sizing to production const (finding 3A)

**Files edited:**
- `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs:41` — added `use crate::render::construction::{PREPARE_PROBE_HISTORY_BYTES, PREPARE_PROBE_HISTORY_ENTRIES};`
- `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs:525-534` — replaced `2048 * 16` literal with `PREPARE_PROBE_HISTORY_BYTES`; replaced `2048 * 4` literal with `(PREPARE_PROBE_HISTORY_ENTRIES * 4) as usize`; rewrote misleading "matches production capacity" comment to point to the const names.

**Diff summary:** Test probe-history buffer now sized via production const SSoT (4 KiB instead of 32 KiB over-allocation); comment updated to factual statement.

**Gate run:** narrow (mechanical const-alignment per architect's design).

**Gate result:**
- `cargo check --workspace`: PASS (clean compile in 1m35s; subsequent reruns ~2-9s).
- `cargo test -p bevy-naadf --lib`: **BLOCKED by pre-existing compile errors at `mod.rs:9725`+`:10831`** — `dispatch_offset` field references that do not exist on the current `GpuGeneratorModelParams` / `GpuEntityUpdateParams` struct definitions. Verified pre-existing by `git stash && cargo test` — same errors at the pre-refactor line numbers `:9747` / `:10853`. Restored my changes via `git stash pop` and continued.

**Status:** PASS (mechanical change applied; cargo check green; lib-test gate blocked by orthogonal pre-existing breakage).

#### Step 2 — Correct the three lying chunks_mirror docblocks (finding 1B + 1C)

**Files edited:**
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:93-105` — replaced layout-entry docblock with present-tense Site 1 text per `03-architecture.md`. The misleading "On native this mirror is allocated to satisfy the layout but never accessed" claim is gone; new text describes the load-bearing-on-both-targets reality, names the read sites (`bounds_calc.wgsl:523` own, `:273` neighbour) and the write site (`:564`).
- `crates/bevy_naadf/src/render/construction/mod.rs:161-168` — replaced `ConstructionGpu.chunks_mirror_buffer` field docblock with Site 2 text. The misleading "but never read on native" claim is gone.
- `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl:117-125` — replaced binding-decl header comment with Site 3 text. Stale line refs (499 / 252 / 538) killed; replaced with anchor-string navigation (`chunks_mirror[`, `chunks[chunk_idx] =`) per finding 1C.

**Diff summary:** Three sites of false "native never accesses chunks_mirror" docblock claim collapsed into accurate present-tense descriptions of the dual cross-frame + intra-pass-cross-workgroup mirror role.

**Gate run:** narrow + 1× native e2e (architect explicitly noted skipping 3× web for comment-only step since web gate runs after step 4 anyway).

**Gate result:**
- `cargo check --workspace`: PASS (2.46s).
- 1× native e2e (`cargo run --release --bin e2e_render -- --vox-horizon-native`): PASS, `[cpu-gpu-parity] ratio=100.000%` (10,460,124 / 10,460,124 same bytes), screenshot saved.

**Status:** PASS.

#### Step 3 — Remove `end_of_encoder_noop_pipeline` dead code (finding 1D)

**Files edited:**
- `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl:432-471` (pre-refactor lines) — deleted the `end_of_encoder_noop` entry point + its 31-line probe-2 narrative header (~36 lines total).
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:290-329` — deleted `queue_end_of_encoder_noop_pipeline` + `_with_handle` + the 10-line probe-2 docblock above them (~40 lines).
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:523-534` — deleted the wasm-only `#[cfg(target_arch = "wasm32")] let Some(end_of_encoder_noop_pipeline) = ... else { return; }` resolution gate (12 lines).
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:670-674` — deleted the wasm-only `#[cfg(target_arch = "wasm32")] { let _ = end_of_encoder_noop_pipeline; }` warning-suppressor block (5 lines).
- `crates/bevy_naadf/src/render/construction/mod.rs:546-555` — deleted the `bounds_calc_pipeline_end_of_encoder_noop` field + its docblock on `ConstructionPipelines` struct.
- `crates/bevy_naadf/src/render/construction/mod.rs:659-669` — deleted the queue site (the `bounds_calc_pipeline_end_of_encoder_noop = bounds_calc::queue_end_of_encoder_noop_pipeline(...)` call).
- `crates/bevy_naadf/src/render/construction/mod.rs:750` — deleted the field initializer in the `ConstructionPipelines` struct-literal at the end of `FromWorld`.

**Diff summary:** Six sites of `end_of_encoder_noop_pipeline` infrastructure removed. The wasm-only early-return-if-pipeline-not-ready hazard (which could block the entire regime-2 node) is gone. Total ≈ 115 lines removed across 3 files; no callers remain (verified by `grep -r end_of_encoder_noop crates/`).

**Gate run:** Full gate sequence per architect (postponed full e2e gate to consolidated final run at end of all related steps per architect's economized verification authorization).

**Gate result (intermediate, post-step 3 cargo check only):**
- `cargo check --workspace`: PASS (9.02s).

**Status:** PASS (full e2e gate consolidated to step 4+5 final run).

#### Step 4 — Extract `refresh_chunks_mirror` helper + collapse body + new function docblock (finding 1F + 1A)

**Files edited:**
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:400-415` — inserted the new private `fn refresh_chunks_mirror(encoder, chunks, chunks_mirror)` helper above the regime-2 node section. Absorbs the `if let (Some, Some) = ...` destructure + `copy_buffer_to_buffer` + `min(src.size(), dst.size())` size clamp that previously appeared twice inline.
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:418-450` (post-edit) — replaced the function-level docblock above `pub fn naadf_bounds_compute_node` with the new present-tense version per architect's design. Lists the load-bearing facts (n_bounds_rounds clamp, mirror seed/refresh semantics, both-targets-same-code, direct-dispatch override orthogonality). Points to `docs/orchestrate/wasm-chunk-aadf-nondeterminism/12-14` for iter-history archaeology.
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:455-475` — collapsed function body. Removed three iter-N narrative blocks at pre-edit `:538-553`, `:572-584`, `:593-625`. Kept `[aadf-probe]` one-shot config log block unchanged (protected instrumentation). Replaced two duplicated `if let (Some, Some) = ...` destructure-copy blocks with two `refresh_chunks_mirror(encoder, chunks_buf_opt.as_ref(), chunks_mirror_buf_opt.as_ref())` calls (seed before round 0, between-rounds inside the loop).

**Diff summary:** Function body collapsed from ~226 lines to ~133 lines. Iter-N archaeology gone from source; `refresh_chunks_mirror` private helper added. Three logical phases (seed / dispatch-loop / between-rounds-refresh) now named at the call site.

**Gate run:** Full gate sequence (deferred from step 3 + this step).

**Gate result:**
- `cargo check --workspace`: PASS (2.11s, post step 4+5 combined).
- `cargo test -p bevy-naadf --lib`: BLOCKED (pre-existing — see overall status caveat 1).
- 2× native e2e (`--vox-horizon-native`):
  - Run 1: `[cpu-gpu-parity] ratio=100.000%` PASS, screenshot saved.
  - Run 2: `[cpu-gpu-parity] ratio=100.000%` PASS, screenshot saved.
- Web build (`just web-build-release`): PASS in 1m29s, `applying new distribution / ✅ success`.
- 3× web e2e (`vox-horizon-parity.spec.ts --headed`):
  - Run 1: SSIM 0.934682 PASS (≥ 0.91) — `target/e2e-screenshots/funnel/vox_horizon_web-20260520T104627-448.txt`
  - Run 2: SSIM 0.928492 PASS (≥ 0.91) — `vox_horizon_web-20260520T104813-277.txt`
  - Run 3: SSIM 0.933220 PASS (≥ 0.91) — `vox_horizon_web-20260520T104858-084.txt`

  None of the runs triggered the panic/RuntimeError/DeviceLost/Browser-closed grep patterns.

**Status:** PASS.

**Notes:** The web e2e required a watchdog workaround (background `while true; do node serve.mjs; sleep 0.5; done`) because the playwright config has a pre-existing race — see caveat 2 + side notes.

#### Step 5 — Drop unused parameters + clean cfg-attrs (finding 1G)

**Files edited:**
- `crates/bevy_naadf/src/render/construction/bounds_calc.rs:455-463` (post-edit) — `naadf_bounds_compute_node` signature: removed `render_device: Res<bevy::render::renderer::RenderDevice>` parameter; removed `render_queue: Res<bevy::render::renderer::RenderQueue>` parameter; removed all three `#[cfg_attr(not(target_arch = "wasm32"), allow(unused_variables))]` annotations (the one on `world_gpu` was outright misleading since `world_gpu` IS used unconditionally at the now-`refresh_chunks_mirror`-call site).
- Verified by `grep RenderDevice|RenderQueue crates/bevy_naadf/src/render/construction/bounds_calc.rs` → no matches; the imports were never present in `bounds_calc.rs`'s `use` block (the parameter type was inlined `Res<bevy::render::renderer::RenderDevice>`), so no import cleanup was needed.

**Diff summary:** Function signature went from 9 parameters to 7. `cfg_attr` annotations gone.

**Gate run:** Combined with step 4 (above). The cargo check + 2× native + 3× web e2e was run after both step 4 and step 5 edits landed, since the architect's design groups them as the load-bearing function-body edits.

**Gate result:** Same as step 4 (all gates green; consolidated run).

**Status:** PASS.

### Final overall verification (consolidated run)

After steps 3, 4, 5 all landed, I ran the full gate sequence per architect's design:

| Gate | Outcome | Detail |
|------|---------|--------|
| `cargo check --workspace` | PASS | Clean compile after each step; final state 2.11s. |
| `cargo test -p bevy-naadf --lib` | BLOCKED (pre-existing) | Errors at `mod.rs:9725` + `:10831` — `dispatch_offset` field. Verified pre-existing on baseline `1fdd256`. |
| 2× native e2e `--vox-horizon-native` | PASS / PASS | Both runs: `[cpu-gpu-parity] ratio=100.000%`. |
| `just web-build-release` | PASS | 1m29s, `applying new distribution / ✅ success`. |
| 3× web e2e SSIM ≥ 0.91 | 3/3 PASS | 0.934682 / 0.928492 / 0.933220 — all ≥ floor. |

Logs in `target/diagnostics/refactor-impl/`:
- `step-1-check.log` / `step-1-test.log` (test log shows pre-existing failure mode)
- `step-2-check.log` / `step-2-native-1.log`
- `step-3-check.log`
- `step-4-5-check.log`
- `final-native-1.log` / `final-native-2.log`
- `final-web-build.log`
- `final-web-1-v2.log` / `final-web-2.log` / `final-web-3.log` (the `-v2` suffix on run 1 marks it as the watchdog-workaround pass; the original `final-web-1.log` shows the pre-existing infra race)

Funnel sidecars (per-run SSIM scores + sentinel data):
- `target/e2e-screenshots/funnel/vox_horizon_web-20260520T104627-448.txt` (run 1: SSIM 0.934682)
- `target/e2e-screenshots/funnel/vox_horizon_web-20260520T104813-277.txt` (run 2: SSIM 0.928492)
- `target/e2e-screenshots/funnel/vox_horizon_web-20260520T104858-084.txt` (run 3: SSIM 0.933220)

### Files modified (consolidated)

| File | Lines (final) | Net delta vs `1fdd256` |
|------|---------------|------------------------|
| `crates/bevy_naadf/src/render/construction/bounds_calc.rs` | 619 | net −145 |
| `crates/bevy_naadf/src/assets/shaders/bounds_calc.wgsl` | 572 | net −33 |
| `crates/bevy_naadf/src/render/construction/mod.rs` | 11043 | net −22 |
| `crates/bevy_naadf/src/render/construction/bounds_calc/tests.rs` | 1014 | net 0 (3 line changes, 1 added import) |

Total per `git diff --stat`: 4 files changed, 105 insertions(+), 248 deletions(-) — net −143 lines.

### Files removed

None at file granularity (all removals were within-file). Six within-file symbol removals via finding 1D:
- WGSL entry point `end_of_encoder_noop` (`bounds_calc.wgsl`).
- Rust pipeline-queue helpers `queue_end_of_encoder_noop_pipeline` + `_with_handle` (`bounds_calc.rs`).
- Struct field `bounds_calc_pipeline_end_of_encoder_noop` (`mod.rs`).
- Pipeline-queue call site in `FromWorld` (`mod.rs`).
- Field initializer in struct literal at end of `FromWorld` (`mod.rs`).
- Wasm-only resolution gate + `let _ =` warning-suppressor in `naadf_bounds_compute_node` (`bounds_calc.rs`).

### Behavioural deltas observed during verification

- Native e2e: `[cpu-gpu-parity] ratio=100.000%` (10,460,124 / 10,460,124 bytes) on both runs — bit-identical to pre-refactor characterization (per `14-cleanup-sweep.md`). No behavioural delta.
- Web e2e SSIM: 0.929 / 0.933 / 0.935 — all in the same cluster as the pre-refactor 10/10 sweep characterization (0.911-0.937, bimodal-ish, median 0.930). All three runs fell in the "comfortable" attractor (≥ 0.92), none in the marginal 0.91x band. No behavioural delta; if anything, slightly more comfortable distribution (3/3 above 0.925) — but with N=3 this is not significant signal.
- Native warning-clean: removing unused `render_device` + `render_queue` parameters silenced two `unused_variables` warnings that the cfg-attr annotations were suppressing.

### Discrepancies between design and source state (if any)

**Discrepancy 1:** Architect's design referenced WGSL line numbers `432-467` for the `end_of_encoder_noop` deletion. Current source had the entry point at WGSL `:432-471` (5-line drift, likely from the architect's text counting the trailing blank line and the next section header differently). Resolved by deleting from the start of the probe-2 narrative comment block through the closing `}` + the following blank line, leaving the next section header intact. No behavioural impact.

**Discrepancy 2:** Architect's design at "Helper extractions" mentions inserting `refresh_chunks_mirror` "in module scope (not `pub`)". The architect's pseudocode shows it as a free function. I inserted it at line ~402 (between `dispatch_regime_2_rounds` and the regime-2 node section comment) as a private (no `pub`) module-scope `fn`. Consistent with design intent.

**Discrepancy 3:** The pre-existing `cargo test -p bevy-naadf --lib` failure (mod.rs `dispatch_offset` field references) was unknown to the architect's design — neither the architect nor explorer flagged it. This is a separate latent issue not introduced by the refactor. Verified by stash-and-baseline-rerun. Reported here for the orchestrator's awareness; not in this refactor's scope.

**Discrepancy 4:** The pre-existing `e2e/playwright.config.ts` + `kill-stale-server.mjs` race (globalSetup runs AFTER webServer.command, killing the server Playwright just started). Verified by running e2e on the baseline commit `1fdd256` source — same failure mode. Worked around with a watchdog respawn loop; not fixed because (a) not in scope, (b) fix requires re-architecting the playwright config which is outside the refactor's target files. Reported for orchestrator awareness.

### Side notes / observations / complaints (MANDATORY per CLAUDE.md)

1. **Two pre-existing breakages were discovered during verification:**
   - **`cargo test -p bevy-naadf --lib` broken on baseline `1fdd256`**: `mod.rs:9725` + `:10831` reference `dispatch_offset` field that doesn't exist on the current `GpuGeneratorModelParams` / `GpuEntityUpdateParams` structs. This is bit-rot — a struct was refactored at some point but two test fixtures inside `mod.rs` weren't updated. The `cargo check` succeeds because these references are inside `#[cfg(test)]` modules; only `cargo test` triggers the test-only compile path that exposes them. Estimated fix: rename `dispatch_offset` to `_pad2` (the available field) or actually compute the proper offset for the test fixture, in both call sites. ~5 minutes' work. Not in refactor scope.
   - **`e2e/playwright.config.ts` + `kill-stale-server.mjs` race on baseline `1fdd256`**: the kill-stale-server globalSetup module — added in commit `1fdd256` itself, per `15-playwright-stale-server-fix.md` — runs AFTER Playwright starts the webServer (`node serve.mjs`), killing it. Tests then run with no server. ERR_CONNECTION_REFUSED at the first `page.goto`. The fix's design intent (kill any cross-worktree squatter BEFORE the new webServer starts) is contradicted by Playwright's actual lifecycle: globalSetup runs after webServer. Verified by running playwright with `DEBUG=pw:webserver` — observe `webServer Starting / Process started / Waiting / WebServer available / [globalSetup] Evicted stale listener(s) / Terminated the WebServer`. The fix is structurally backwards. Estimated fix: move the kill-stale logic OUT of globalSetup and INTO the webServer command itself (e.g. `command: "node serve.mjs"` becomes `command: "bash -c 'lsof -ti :4173 | xargs -r kill -9 || true; node serve.mjs'"`). Or: drop globalSetup entirely and rely on `reuseExistingServer: false` in CI mode + manual cleanup locally. ~10 minutes' work. Not in refactor scope, but **highly recommend the orchestrator escalate this** — without fixing it the next refactor session also can't verify against the e2e gate without the watchdog hack.

2. **The watchdog hack used for web e2e verification:**
   `cd e2e && nohup bash -c 'while true; do node serve.mjs 2>&1; sleep 0.5; done' > /tmp/watchdog-serve.log 2>&1 &`
   Started before each `npx playwright test` invocation. The globalSetup kills the listener, the watchdog respawns within 0.5s, Playwright's webServer block sees a listener (via `reuseExistingServer: !CI` = true locally) and the test proceeds. All 3 web runs PASS via this workaround. The watchdog is killed via `pkill -9 -f "node serve.mjs"` between dispatches. Documented here so the orchestrator can reproduce; do not commit the watchdog to the repo.

3. **The refactor delivered well — −143 net lines, ~50% smaller `naadf_bounds_compute_node` body (~226 → ~133 lines), three lying docblocks rewritten to match code, dead `end_of_encoder_noop_pipeline` infrastructure eliminated.** The architect's design was precise: every file:line reference held up under `Read` verification (one 5-line drift on the WGSL deletion range, harmless). The migration steps ordered risk-correctly (mechanical first, comment-only next, then dead-code, then helper-extract, then signature cleanup) — no step left the tree non-buildable.

4. **Subjective: pacing felt right for an Opus-tier refactor.** The reading-required-files step alone took ~10k tokens (01-context.md + 02-exploration.md + 03-architecture.md are dense, but they did the design work upfront — no architectural decisions left for the implementer, which is the right scope-shape for this phase). The actual edits were ~30 minutes wall-clock (steps 1-5) and the verification gate ~25 minutes (cargo check is ~10s once warm, native e2e is ~3s × 2 = 6s once binary is cached, web-build is ~90s, web e2e is ~1m × 3 = 3m). The big time sinks were diagnosing the two pre-existing breakages (~15 min) and the watchdog workaround (~5 min).

5. **Foundation rot flag (smell-driven escape, per global CLAUDE.md):** The `mod.rs` file is 11,043 lines. The `dispatch_offset` test-fixture bit-rot exists because `mod.rs` has so many `#[cfg(test)]` islands that a routine struct refactor missed two of them. The W3 module shape is fine — but `mod.rs` as the host for `ConstructionGpu` + `ConstructionPipelines` + `ConstructionBindGroups` + 4-5 W-stage test fixtures + the `[probe1-call]` ring infrastructure is exactly the "single giant file" pattern that breeds bit-rot. Future refactor candidate: split `crates/bevy_naadf/src/render/construction/mod.rs` into thematic submodules (`pipelines.rs`, `gpu_state.rs`, `bind_groups.rs`, `probe_history.rs`). Out of scope for this dispatch.

6. **Architect's call on Item 2A (ESCAPE, not SMALL+OBVIOUS+LOW-RISK) holds up under verification.** Even after my refactor (which keeps the chunks-RMW pattern identical), the web e2e parity is 0.929-0.935 — in the 0.91-0.94 cluster the architect predicted, not the 80%+ the explorer's Option B target would aim for. The "natural follow-up `/refactor` for Option B" recommendation is correct: this is a separate algorithmic restructure, not a comment-cleanup sweep.

7. **The lib-test breakage means I could not directly verify the test-const-alignment win from Step 1.** The `2048 * 16` → `PREPARE_PROBE_HISTORY_BYTES` replacement compiles (cargo check passes), the import is correct (verified via `cargo check`), but I couldn't actually RUN the affected test (`bounds_calc_convergence_matches_cpu_oracle`) because the unrelated `mod.rs:9725` errors blocked the whole lib-test compile. The change is mechanically equivalent (4 KiB > shader's `arrayLength(...)/4u`-derived count of 256 = same effective ring size as before), and `cargo check` proves the new constants are reachable from the test's import path — but a future "fix the dispatch_offset bit-rot" session should re-run the bounds_calc tests to confirm. Logging this so the orchestrator can chain the two refactors.

8. **The architect's "anchor strings instead of line numbers" decision in finding 1C is a small but high-leverage win.** The new WGSL header comment says "search `chunks_mirror[` to find the read sites" — `grep` finds them in ~5ms regardless of file drift. The old "line 499 / 252 / 538" refs drift on every commit. This is the kind of code-comment pattern that should propagate to other parts of the codebase; future explorer dispatches might surface it as a positive example.
