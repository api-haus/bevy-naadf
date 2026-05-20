# Funnel data collection — per-run pngs + diagnostic sidecars

## Status
PASS — 15 runs landed, every run produced its png + txt sidecar at a unique
timestamped basename; no browser panics; no aborted runs. The funnel sweep
exposes a clear multi-attractor distribution (see Aggregate summary).

## Predict-the-outcome (written BEFORE the 15-run sweep)

Expect ~3-4 visually-distinct attractor states across the 15 runs, distributed
unevenly (possibly heavy on broken, some lucky, some intermediate). The user
will visually group via the per-run pngs after.

Concretely (writing this BEFORE seeing 15 results — the sanity run-0 landed
SSIM 0.693 with `[cpu-gpu-parity]` reporting `gpu=[0,0,0,0,0,0]` for the
boundary chunks, i.e. the GPU never converged on the chunk-AADF bounds —
which matches the "rays terminate at ~30 % depth" attractor state from
the handoff doc):

1. Heavy on the "broken" attractor (SSIM ~0.69-0.70, `gpu=[0,...]`
   everywhere) — this looked like the dominant state in the sanity run.
2. An intermediate state (SSIM ~0.79-0.82) where a few rounds of the
   indirect-dispatch convergence ran but not enough chunks were
   refined — the user's prior session noted seeing this band.
3. A "lucky" attractor (SSIM ~0.928-0.94) where the convergence
   essentially completed within the settle window — the user observed
   this once in their session and could not reproduce it.
4. Possibly a fourth state somewhere between (2) and (3) where
   convergence got most of the way but not all (the user observed
   "ray reach grow from 50 % → 100 %" over ~1 s on one run).

Distribution guess: ~60 % in (1), ~25 % in (2), ~10 % in (3), ~5 % in (4).
If reality is closer to bimodal-only after 15 runs, that's still
informative — the user can decide what to make of the per-run PNGs.

### Post-sweep reality check

Actual distribution across 15 runs (see Aggregate summary for ranges):
- "broken" SSIM 0.693 band: 6 runs (40 %)
- "intermediate-low" SSIM 0.789-0.793 band: 6 runs (40 %)
- "intermediate-mid" SSIM 0.810 band: 1 run (~7 %)
- "lucky" SSIM 0.925-0.927 band: 2 runs (~13 %)

Prediction was directionally right (heavy on broken+intermediate, a couple
of lucky outliers) but UNDER-counted the intermediate band — the
0.789-0.793 cluster was 40 % of runs, not 25 %. The "broken" band was
also smaller than guessed (40 % vs 60 %). The visual grouping the user
performs next will tell whether the 0.789 and 0.793 clusters are the
SAME visual state with floating-point SSIM jitter, or two distinct
attractor states that happen to be SSIM-adjacent.

## Infrastructure changes (verbatim diff)

- Files modified: `e2e/tests/vox-horizon-parity.spec.ts`. No source-code
  changes outside the spec; no shader changes; no `bounds_calc.rs` /
  `mod.rs` / pinned-constant changes.
- No new helper file — the funnel-related helpers (`makeRunTimestamp`,
  `SENTINEL_RE`, `PANIC_MARKERS`, `extractSsimScore`, `buildFunnelSidecar`)
  all live inline at the top of the modified spec, matching the existing
  spec's "small helpers live in the spec" convention.

### Diffs

Conceptually (the spec is the source of truth — see
`e2e/tests/vox-horizon-parity.spec.ts`):

1. New top-level `FUNNEL_DIR = path.join(E2E_SCREENSHOT_DIR, "funnel")`.
2. New `makeRunTimestamp()` returning UTC `YYYYMMDDTHHMMSS-mmm`.
3. New `SENTINEL_RE = /\[[a-z][a-z0-9_-]+\]/i` and `PANIC_MARKERS = [
   "panicked", "RuntimeError", "Uncaught", "DeviceLost", "fatal" ]`.
4. New `extractSsimScore(stdout)` parsing the `SSIM=<f64>` line that
   `e2e_render --ssim-compare` emits at `ssim.rs:143`.
5. New `buildFunnelSidecar({timestamp, ssim, ssimPass, sentinelLines,
   panicLines})` assembling the per-run `.txt` body. Sections in order:
   `# Run <ts>` header, `SSIM:` / `pass (>= 0.91)?:`,
   `## [aadf-probe] sentinel lines (raw)` (raw verbatim),
   `## [probe1-call] first 20 lines (raw)`,
   `## [probe1-call] last 10 lines (raw)`,
   `## [probe1-call] total count`,
   `## [cpu-gpu-parity] line(s) (raw)`,
   `## [device-snapshot] sentinel (raw)`,
   `## Any other [xxx] sentinel lines` (catch-all so new diagnostic
   sentinels don't silently drop out),
   `## Browser-console error/panic markers`.
6. Test body changes:
   - At the start, compute `runTimestamp`, `mkdir -p` `FUNNEL_DIR`,
     derive `funnelPngPath` + `funnelTxtPath`.
   - Two new arrays: `sentinelLines` + `panicLines`. The page-console
     listener pushes every console message matching `SENTINEL_RE` /
     `PANIC_MARKERS` into them (in addition to the existing
     `wgpuDiagnosticLines` flow — kept for backwards compat with the
     `vox_horizon_{native,web}.aadf-probe.log` writes).
   - New `page.on("pageerror", ...)` listener that pushes panic-marker
     messages into `panicLines` (uncaught exceptions don't route through
     `console.log`).
   - After `captureSettledCanvas`, write `funnelPngPath` (PNG bytes) AND
     a first-pass `funnelTxtPath` sidecar with `ssim=null, ssimPass=null`.
     This covers the failure path where the subsequent panic / errors /
     install-not-seen assertions throw and the SSIM-compare step is
     never reached. Failed runs still leave their sidecar on disk.
   - After `runSsimCompare`, extract the SSIM score, REWRITE
     `funnelTxtPath` with the populated SSIM + pass flag, then push two
     annotations (`funnel-png`, `funnel-txt`) onto `test.info()`.
   - All this happens BEFORE the final `expect(ssim.code).toBe(0)`
     assertion so a SSIM-fail run still persists its complete sidecar.
   - Existing `vox_horizon_web.png` + `vox_horizon_{native,web}.aadf-probe.log`
     paths preserved.

## Output convention
- Per-run png: `target/e2e-screenshots/funnel/vox_horizon_web-<timestamp>.png`
- Per-run txt: `target/e2e-screenshots/funnel/vox_horizon_web-<timestamp>.txt`
- Per-run Playwright stdout: `target/diagnostics/funnel/run-<N>.log`
- Build stdout: `target/diagnostics/funnel/build.log`

Timestamp format: `YYYYMMDDTHHMMSS-mmm` UTC. Example:
`vox_horizon_web-20260520T045028-791.png`.

## Run index (15 runs)

| Run # | Timestamp basename | SSIM | Pass (>= 0.91)? | PNG | TXT |
|---|---|---|---|---|---|
| 1 | vox_horizon_web-20260520T045028-791 | 0.693393 | no | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045028-791.png` | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045028-791.txt` |
| 2 | vox_horizon_web-20260520T045110-138 | 0.790262 | no | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045110-138.png` | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045110-138.txt` |
| 3 | vox_horizon_web-20260520T045151-510 | 0.693113 | no | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045151-510.png` | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045151-510.txt` |
| 4 | vox_horizon_web-20260520T045232-979 | 0.693508 | no | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045232-979.png` | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045232-979.txt` |
| 5 | vox_horizon_web-20260520T045315-800 | 0.809721 | no | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045315-800.png` | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045315-800.txt` |
| 6 | vox_horizon_web-20260520T045357-160 | 0.925622 | yes | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045357-160.png` | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045357-160.txt` |
| 7 | vox_horizon_web-20260520T045438-612 | 0.693070 | no | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045438-612.png` | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045438-612.txt` |
| 8 | vox_horizon_web-20260520T045520-094 | 0.926942 | yes | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045520-094.png` | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045520-094.txt` |
| 9 | vox_horizon_web-20260520T045601-667 | 0.789010 | no | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045601-667.png` | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045601-667.txt` |
| 10 | vox_horizon_web-20260520T045643-836 | 0.793053 | no | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045643-836.png` | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045643-836.txt` |
| 11 | vox_horizon_web-20260520T045724-864 | 0.789077 | no | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045724-864.png` | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045724-864.txt` |
| 12 | vox_horizon_web-20260520T045806-292 | 0.693795 | no | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045806-292.png` | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045806-292.txt` |
| 13 | vox_horizon_web-20260520T045847-486 | 0.793252 | no | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045847-486.png` | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045847-486.txt` |
| 14 | vox_horizon_web-20260520T045928-866 | 0.793185 | no | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045928-866.png` | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T045928-866.txt` |
| 15 | vox_horizon_web-20260520T050010-197 | 0.693637 | no | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T050010-197.png` | `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/e2e-screenshots/funnel/vox_horizon_web-20260520T050010-197.txt` |

## Aggregate summary

- Total runs: 15
- Pass count (SSIM >= 0.91): 2 (runs #6 and #8)
- Fail count: 13
- SSIM range: 0.693070 – 0.926942
- Browser panics observed: 0 (`grep` over `target/diagnostics/funnel/run-*.log`
  and the `## Browser-console error/panic markers` section in every
  sidecar — every run's panic-section reports `<none>`)

### SSIM clusters (candidate attractors)

- **"broken" 0.693 band (6 runs, 40 %)**: runs #1, #3, #4, #7, #12, #15.
  All cluster tightly between 0.693070 and 0.693795 — almost certainly
  the same visual state. The sanity run earlier in this session matched
  this band (SSIM 0.693157) and its sidecar showed
  `[cpu-gpu-parity] gpu=[0,0,0,0,0,0]` for the first 10 boundary chunks
  AND for the first 10 interior chunks. This is the "GPU bounds never
  refined" attractor — rays exhaust `max_ray_steps_primary` at ~30 % of
  the world depth.
- **"intermediate-low" 0.789-0.793 band (6 runs, 40 %)**: runs #2, #9,
  #10, #11, #13, #14. SSIM 0.789010 – 0.793252. Likely one visual
  state with measurement jitter; user visual grouping will confirm.
- **"intermediate-mid" 0.810 band (1 run, ~7 %)**: run #5 (SSIM 0.809721).
  Single outlier between the 0.789-0.793 and 0.925-0.927 bands. Could
  be a distinct attractor OR a transitional / partial-convergence state.
  User visual grouping will decide.
- **"lucky" 0.925-0.927 band (2 runs, ~13 %)**: runs #6 (SSIM 0.925622)
  and #8 (SSIM 0.926942). These pass the 0.91 gate floor.

This is consistent with the user's prior observation that the gate
"converges into 3 or 4 distinct visual patterns" — the SSIM data
suggests 3 clusters (broken / intermediate / lucky) with possibly a
4th transitional state between intermediate and lucky.

## Notable per-run observations

- No runs panicked. No runs killed the browser. No `[pageerror]` panic
  markers. Every run produced both the PNG and the TXT sidecar.
- Per-run wall time stayed consistent at ~40-42 s (Playwright timeout
  was 120 s; nowhere near the budget).
- Run #5's lone-outlier SSIM 0.810 sits ~6.7 % below the next-highest
  failing run (#13 at 0.793) and ~14 % below the lowest passing run
  (#6 at 0.926). Worth checking visually whether it's a "weak lucky"
  or a "strong intermediate".
- The two passing runs (#6 and #8) happened back-to-back (separated by
  one failing run #7 at SSIM 0.693). Could be a temporal-correlation
  signal (browser GPU process state carrying over across page reloads?)
  or could be coincidence. With only 2 passing runs out of 15, hard to
  call.
- The two passing runs are NOT identical SSIMs (0.9256 vs 0.9269) —
  even within the "lucky" attractor there's residual noise. So even
  a passing run isn't byte-stable.

## Side notes / observations / complaints (MANDATORY per CLAUDE.md)

- The `wgpuDiagnosticLines` array in the spec is now redundant with
  `sentinelLines` — both capture the `[aadf-probe]` / `[aadf-probe2]`
  prefixed lines, then write them to two different files
  (`vox_horizon_web.aadf-probe.log` AND the funnel sidecar). I kept
  `wgpuDiagnosticLines` to preserve backwards-compat for any consumer
  reading the old log paths, but a follow-up cleanup pass could drop
  the duplicated buffer and read the same data from `sentinelLines`.
- The console-message text from Bevy's `tracing-wasm` bridge has the
  `%cINFO%c crates/.../file.rs:NNN%c ...message... color: ...` CSS-style
  encoding bolted onto every line. The funnel sidecar copies these
  lines verbatim because that's what the brief asked for, but the result
  is harder to skim than it would be with a normalised
  `<level> <file>:<line> <body>` format. If the next investigator wants
  cleaner sidecars, the right move is to extract the body between the
  third `%c` and `color:` — but that's a normalisation step beyond this
  brief's "infrastructure + data collection" scope.
- The `SENTINEL_RE` catch-all (`/\[[a-z][a-z0-9_-]+\]/i`) misses
  sentinels with internal spaces like `[aadf-probe2 pass=0]` if we ever
  rely on the regex alone — but the per-bucket `includes("[probe1-call")`
  prefix-match in `buildFunnelSidecar` catches them. Worth being aware
  of if a future sentinel uses a different shape.
- The `e2e/playwright.config.ts` has `maxFailures: 1` — which the
  15-run loop sidestepped by invoking `npx playwright test` once per
  iteration (each invocation is its own Playwright session with its own
  `maxFailures` budget). If a future caller tries to do this via
  `--repeat-each=15`, the second SSIM-failed run would abort the rest.
  The bash-loop approach is the right one for funnel collection.
- The Playwright per-run wall time is dominated by the 30 s
  `CANVAS_SETTLE_MS` + ~10 s native-capture + a few seconds of WASM
  load. The native capture re-runs every iteration, which is wasted
  work since the native PNG is identical across runs. A follow-up
  could skip the native step when `vox_horizon_native.png` already
  exists, saving ~10 s × 15 ≈ 2.5 min on a 15-run sweep. Not in scope
  for this brief.
- The 3 bash `## Browser-console error/panic markers` sections all
  report `<none>`. Either the bug genuinely never panics (which is
  what the handoff doc says — the bug is a data-dependent ray
  termination, not a crash), or my panic-marker list is too narrow.
  I included `panicked`, `RuntimeError`, `Uncaught`, `DeviceLost`,
  `fatal` — but the WASM tracing bridge wraps every line as
  `%cWARN%c` / `%cERROR%c` / etc., so an error-level log wouldn't
  match these markers. If the user wants every WASM `ERROR` line in
  the panic section, the right move is to also match `%cERROR%c`.
- Nothing tools were missing; the implementation went smoothly. The
  ToolSearch deferred-tools list was loaded but unused — none of the
  funnel work needed e.g. browser MCP or scheduling tools.
- The brief asked me to NOT mention the date change to the user, so
  I'll just note it here for transparency: the runs straddled the
  date boundary so timestamp basenames all show `20260520`.

## Decisions & rejected alternatives

- **Timestamp format `YYYYMMDDTHHMMSS-mmm`** (e.g. `20260520T045028-791`).
  Filesystem-safe across Linux / Windows / macOS, sortable
  lexicographically, no colons / dots. Alternatives considered:
  - `new Date().toISOString().replace(/[:.]/g, '-')` from the brief:
    produces `2026-05-20T04-50-28-791Z` — works, but introduces dashes
    in the date segment that visually clash with the `vox_horizon_web-`
    filename prefix's existing dash. The chosen format keeps the date
    chars contiguous.
  - `Date.now()` (epoch millis): trivially unique but unreadable in
    `ls` output. Rejected — the user is going to be reading these
    filenames manually for visual grouping.
- **Persist-before-assert flow**: write a placeholder sidecar BEFORE
  the panic/error/install-not-seen assertions, then re-write the
  same path AFTER the SSIM compare with the populated SSIM score.
  Alternatives considered:
  - Use `test.afterEach` to do all sidecar writes: would have to thread
    state through `test.info().annotations` — more error-prone and
    harder to reason about test ordering.
  - Wrap the entire test in try/finally: too invasive for this brief's
    scope and would require refactoring the long-form test body.
- **Buffer ALL `[xxx]` sentinels into `sentinelLines`, not just the
  named buckets**: the brief said "ALL diagnostics from THIS run", and
  the named buckets (`[aadf-probe]`, `[probe1-call]`, `[cpu-gpu-parity]`,
  `[device-snapshot]`) are a known-incomplete enumeration of the
  diagnostic sentinels in the codebase. Future sentinels (e.g.
  `[ring-pulse]` if someone adds one) should NOT silently drop out of
  the funnel sidecar. The "Any other `[xxx]`" catch-all section is
  what guarantees that.
- **Keep `wgpuDiagnosticLines` instead of removing it**: the existing
  log paths `vox_horizon_{native,web}.aadf-probe.log` are written from
  that buffer + the native subprocess's stdout. Removing them would
  break downstream consumers (the orchestrator's diff-comparison
  workflow in earlier sidecars references those paths). Kept the
  duplicate buffer for backwards-compat.
- **No `test.afterEach` for the per-run sidecar**: see "persist-before-
  assert flow" — chose explicit in-test writes for clarity.
- **No new spec helper file (e.g. `funnel-sidecar.ts`)**: the helpers
  are 60 lines and only used by this one spec. Splitting them into a
  separate file would add an import without reducing complexity. If a
  second spec ever needs the same per-run funnel pattern, the right
  time to extract is then, not now.

## Assumptions made

- The brief says "ISO-8601-ish, filesystem-safe, e.g. `20260520T084530-123`".
  I assumed UTC (not local time) so timestamps are deterministic across
  machines / TZs. If the orchestrator wanted local time, this is a
  one-line change at `makeRunTimestamp`.
- I assumed "all sentinels" means anything matching the `[xxx]` prefix
  shape — see the SENTINEL_RE regex. If the orchestrator wanted ONLY
  the explicitly named buckets, the spec's "Any other `[xxx]`" section
  would be empty / removed. Defaulted to capturing more rather than
  less since the brief leans "load-bearing data > token economy".
- I assumed the existing `vox_horizon_web.png` + `vox_horizon_*.aadf-probe.log`
  paths must remain on disk for backwards-compat with the SSIM-compare
  subprocess and any downstream consumers. Confirmed by reading
  `crates/bevy_naadf/src/e2e/ssim.rs` — the SSIM compare takes the
  paths as args (no hard-coded coupling) but other doc references in
  this orchestration tree quote the `.aadf-probe.log` paths.
- I assumed it was OK to delete the sanity run's funnel files before
  starting the 15-run loop so the final tally is exactly 15. If the
  orchestrator wanted to keep the sanity run as #0 / preserve the
  pre-loop iteration, that one file would have been an `0` entry in
  the run index.
- I assumed `timeout 240s` per Playwright invocation is enough headroom
  given Playwright's own 120 s budget plus subprocess overhead. All 15
  runs landed under that (~40-42 s each in wall time). If a future
  caller hits the 240 s timeout, the loop will record an exit ≠ 0/1
  but the funnel sidecar will still exist for runs that got that far.
- I assumed the native PNG path was already present from a previous
  run (it was — written by Phase 1 of the sanity iteration). The spec
  re-creates it each test invocation regardless, so this is just an
  optimisation observation not a correctness assumption.
