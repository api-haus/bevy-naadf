# Diagnostics-package implementation log

## Status
PARTIAL@step-4

## Source-code edits
None this dispatch. Diagnostic package source committed earlier as
`6cf4746`; web-time fix as `6cf4746`; web-time hyphen fix as `f1a19c4`.
This dispatch is data-collection only.

## Step-by-step

### 1. Native release build
- Command: `timeout 1500s cargo build --release --bin e2e_render`
- Exit: 0 | Wall: 3m 00s (per `cargo` Finished line)
- Log: `target/diagnostics/logs/01-cargo-build-release.log` (595 bytes)
- Last 3 lines:
  ```
     Compiling bevy v0.19.0-rc.1
     Compiling bevy-naadf v0.1.0 (/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/crates/bevy_naadf)
      Finished `release` profile [optimized] target(s) in 3m 00s
  ```
- Error grep (`error\[E[0-9]+\]|^error:`): none
- `target/release/e2e_render`: present, 185 236 672 bytes,
  mtime `2026-05-19 21:21:55 +0300`.

### 2. Native snapshot
- Command: `timeout 180s just diag-native`
- Exit: 0
- Log: `target/diagnostics/logs/02-diag-native.log` (13693 bytes)
- Output JSON: `target/diagnostics/device-snapshot-native.json` (8871 bytes,
  mtime `2026-05-19 21:22:11 +0300`)
- Last 5 lines:
  ```
  [aadf-probe] regime-2 config: n_bounds_rounds=5 max_group_bound_dispatch=32768 (the wasm clamp ceiling is 4096)
  e2e_render: screenshot saved to target/e2e-screenshots/e2e_latest.png
  e2e_render: luminance gate (batch 6) — 100.0% of the frame is non-black (luminance > 2); threshold 95%
  e2e_render: region luminance — emissive 247.6, solid(GI-lit diffuse) 243.6, sky 202.9  (solid is the TAA camera-motion decay tripwire — the readback is post-camera-motion, so solid should stay >= 150; a decay collapses it toward ~4-6)
  e2e_render: PASS (batch 6) — 96 warmup + 48 camera-motion + 1 settle frames, framebuffer read back & non-degenerate, per-batch region gate green through camera motion, every pipeline created cleanly, every expected render-graph node dispatched.
  ```
- Panic grep (`panicked|^error|FATAL`): none

### 3. Web build
- Command: `timeout 1500s just web-build-release`
- Exit: 0 | Wall: 9.30s cargo + ~27s trunk total (per `cargo` Finished + trunk timestamps)
- Log: `target/diagnostics/logs/03-web-build.log` (2732 bytes)
- Dist wasm: `2026-05-19 21:22:59.720412350 +0300 114651933 crates/bevy_naadf/dist/bevy-naadf-20c65a6ebc74485c_bg.wasm`
- Error grep (`error\[E[0-9]+\]|^error:|panicked|FATAL`): none
- Note: trunk-side warnings present (unstable `atomics`, unused imports,
  unreachable statement in `bounds_calc.rs:472`, unused `mut`/vars) — all
  pre-existing in the source tree, not new.

### 4. Web snapshot — FAILED
- Command: `cd e2e && timeout 240s npx playwright test device-snapshot.spec.ts --headed`
- Exit: 0 (Playwright wrapper exits 0; the test itself FAILED)
- Log: `target/diagnostics/logs/04-diag-web.log` (2644 bytes)
- Output JSON: `target/diagnostics/device-snapshot-web.json` — **NOT
  WRITTEN** (test threw before `fs.writeFile`).
- Wall: 3.1s for the failing case.
- Failure mode (verbatim from log):
  ```
  Error: device-snapshot line is not valid JSON: Unexpected non-whitespace character after JSON at position 5730 (line 1 column 5731)
  line: {"schema_version":1,"target":"web","captured_at_unix_seconds":1779214992,"adapter_info":{"name":"NVIDIA GeForce RTX 5080","vendor":0,"device":0,"device_pci_bus_id":"","driver":"","driver_info":"","backend":"browserwebgpu","device_type":"Other","subgroup_min_size":4,"subgroup_max_size":128,"transient_saves_memory":false},"adapter_features":["bgra8unorm-storage","clip-distances","depth-clip-control","depth32float-stencil8","dual-source-blending","float32-blendable","float32-filterable","indirect-f
  ```
- Test reporter line preceding the throw:
  `[device-snapshot.spec] captured snapshot line (5812 bytes)` — i.e. the
  spec captured a 5812-byte string and `JSON.parse` choked at position
  5730 (column 5731). The JSON body itself terminates at byte 5730; the
  trailing 82 bytes after position 5730 are the "non-whitespace
  character" the parser refused.
- Browser-console grep against the Playwright log
  (`panicked|RuntimeError|Uncaught|DeviceLost|fatal|Browser closed|Test timeout`):
  none.
- Playwright per-test artefacts written by the failing case:
  - `e2e/test-results/device-snapshot-WASM-devic-5025a-inel-and-write-JSON-to-disk-chromium/error-context.md`
  - `e2e/test-results/device-snapshot-WASM-devic-5025a-inel-and-write-JSON-to-disk-chromium/trace.zip`
  - shared trace bundle:
    `e2e/test-results/.playwright-artifacts-0/traces/63a3c623cc2b2b06ff3e-04a6200302ce4b5d1c67.trace`
    (+ `.network`, + `-pwnetcopy-1.network`, + `resources/`)
  - last-run summary: `e2e/test-results/.last-run.json`
- `error-context.md` body in full:
  ```
  # Page snapshot

  ```yaml
  - generic [ref=e5]: Downloading default model…
  ```
  ```
  (i.e. at the moment the snapshot line was captured the page banner
  still said "Downloading default model…" — the WASM app emitted the
  device-snapshot sentinel before the model finished loading. Reported
  raw, no diagnosis.)

### 5. Compare — NOT EXECUTED
Per dispatch brief: "ANY failure → STOP, write impl log with current
state, return. Don't retry, don't investigate, don't continue to the
next step." Step 4 failed → step 5 skipped.
- `target/diagnostics/logs/05-diag-compare.log` — not produced.

## Comparison output (FULL, verbatim)
```
NOT EXECUTED — step 4 failed before web snapshot JSON could be written.
The native JSON exists but the web JSON does not, so `diag_compare`
would exit 2 (missing/malformed inputs) by design. The dispatch
brief forbids running step 5 after a step-4 failure.
```

## Artifacts on disk (absolute paths)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/device-snapshot-native.json` (8871 bytes)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/device-snapshot-web.json` — **MISSING**
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/logs/01-cargo-build-release.log` (595 bytes)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/logs/02-diag-native.log` (13 693 bytes)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/logs/03-web-build.log` (2732 bytes)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/logs/04-diag-web.log` (2644 bytes)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/target/diagnostics/logs/05-diag-compare.log` — **NOT PRODUCED**
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/e2e/test-results/device-snapshot-WASM-devic-5025a-inel-and-write-JSON-to-disk-chromium/error-context.md`
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/e2e/test-results/device-snapshot-WASM-devic-5025a-inel-and-write-JSON-to-disk-chromium/trace.zip`
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/e2e/test-results/.playwright-artifacts-0/traces/63a3c623cc2b2b06ff3e-04a6200302ce4b5d1c67.trace`
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/e2e/test-results/.playwright-artifacts-0/traces/63a3c623cc2b2b06ff3e-04a6200302ce4b5d1c67.network`
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/e2e/test-results/.playwright-artifacts-0/traces/63a3c623cc2b2b06ff3e-04a6200302ce4b5d1c67-pwnetcopy-1.network`
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/e2e/test-results/.playwright-artifacts-0/traces/resources/` (resource bundle, many files)
- `/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/e2e/test-results/.last-run.json`

## Anomalies observed (raw, no diagnosis)
- The web-side `[device-snapshot]` sentinel line captured by the spec is
  5812 bytes; `JSON.parse` rejects it at position 5730 column 5731 with
  "Unexpected non-whitespace character after JSON". I.e. ~82 bytes of
  non-whitespace trailing payload after the JSON body's closing brace.
  Native sentinel line — same emitter, different target — parses
  cleanly and `device-snapshot-native.json` is 8871 bytes.
- The Playwright `error-context.md` page snapshot at the moment of the
  spec failure shows the page DOM still displaying
  "Downloading default model…" — the WASM app emitted the device-
  snapshot sentinel before the default-model download finished.
- Web adapter_info shows `vendor:0 device:0 driver:"" driver_info:""
  device_pci_bus_id:"" device_type:"Other"
  backend:"browserwebgpu"` — Chrome/Dawn does not surface those fields
  to wgpu, even though the underlying GPU is the same NVIDIA RTX 5080
  that the native snapshot identifies fully. (Visible in the captured
  500-byte prefix of the failing line in the Playwright log.)
- Web subgroup sizes (`subgroup_min_size:4 subgroup_max_size:128`)
  differ from native (`32 / 32`). Raw observation; no diagnosis.
- Web build emitted compile warnings the native build did not (unstable
  `atomics`, unreachable statement at `bounds_calc.rs:472`, unused
  imports/mut/vars). These are pre-existing in the source tree, not
  introduced by this dispatch.
- Native release build was a fresh cold-cache compile (3 m 00 s) — the
  prior dispatches' artefacts were not present.
- Native snapshot JSON contains `"git_sha":"unknown"` in the `build`
  block — the snapshot's git-SHA capture path returns the literal
  string "unknown" rather than the worktree's actual SHA (`f1a19c4`).
