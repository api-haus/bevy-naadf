# Diagnostics-package implementation log

## Status
FAILED at Step 1 — `cargo check --workspace` rejects the orchestrator's
`web_time` fix because the crate is published as `web-time` (hyphen), not
`web_time` (underscore). Cargo treats them as different package names and
the registry lookup fails. **No further steps were attempted, per the
brief's "NO retries on failure" rule.**

## Source-code edits
None. The diagnostic package was implemented by the design-phase dispatch
(see `01-diagnostics-design.md`); the orchestrator applied a follow-up
`web_time` fix outside this dispatch. This dispatch is data-collection only.

## Step-by-step
### 1. cargo check
- Command: `timeout 120s cargo check --workspace 2>&1 | tee target/diagnostics/logs/01-cargo-check.log`
- Result: cargo exited non-zero with a registry-resolution error before
  any compilation began. (Note: the bash pipe's overall exit is `tee`'s
  exit, which masks cargo's non-zero status. The error keyword still
  matches the stop condition.)
- Log: `target/diagnostics/logs/01-cargo-check.log` (294 bytes)
- Full log contents (verbatim):

  ```
      Updating crates.io index
  error: no matching package named `web_time` found
  location searched: crates.io index
  required by package `bevy-naadf v0.1.0 (/mnt/archive4/DEV/bevy-naadf/.claude/worktrees/wasm-chunk-aadf-determinism/crates/bevy_naadf)`
  help: packages with similar names: web-time
  ```

- Root cause: `crates/bevy_naadf/Cargo.toml:110` declares
  `web_time = "1"`. The crates.io package is named `web-time`. Cargo
  package names disallow underscores in published-name resolution; the
  underscore form is a different identifier and resolves to nothing.
- Stop condition matched: the brief's Step-1 rule "If `error[` appears
  in the log, STOP." The leading-token `error:` (no bracket — top-level
  cargo error, not a rustc diagnostic code) is the same class of failure
  signal and triggers the same stop. The brief's wider "any command
  returns non-zero" rule also applies (cargo's true exit is non-zero;
  bash pipe-exit hides it).

### 2. Native snapshot
Not attempted (Step 1 failed).

### 3. Web build
Not attempted (Step 1 failed).

### 4. Web snapshot
Not attempted (Step 1 failed).

### 5. Comparison
Not attempted (Step 1 failed).

## Comparison output (full)
Not produced. `just diag-compare` was never invoked because the workspace
no longer compiles, so neither snapshot can be regenerated against the
current source.

## Artifacts on disk
- `target/diagnostics/logs/01-cargo-check.log` — 294 bytes, full failure
  output captured at the start of this dispatch.
- `target/diagnostics/device-snapshot-native.json` — **8869 bytes,
  mtime 2026-05-19 20:14:28** — a pre-existing native snapshot from a
  prior dispatch attempt. NOT produced by this dispatch and NOT against
  the current `Cargo.toml` (the `web_time` typo blocks the workspace
  from compiling, so this file is necessarily from a build before the
  fix was attempted). Header preview:
  ```json
  {"schema_version":1,"target":"native","captured_at_unix_seconds":1779210868,
   "adapter_info":{"name":"NVIDIA GeForce RTX 5080","vendor":4318,"device":11266,
   "device_pci_bus_id":"0000:01:00.0","driver":"NVIDIA","driver_info":"595.71.05",
   "backend":"vulkan","device_type":"DiscreteGpu",…
  ```
  Treat with caution — it represents the pre-fix codebase, not the
  current branch tip.
- `target/diagnostics/device-snapshot-web.json` — DOES NOT EXIST. No
  web snapshot has ever been produced.
- `e2e/test-results/` — does not exist (Playwright never ran).

## Anomalies observed (raw — do NOT diagnose)
- `crates/bevy_naadf/Cargo.toml:110` reads `web_time = "1"` — the
  underscore form. The crates.io package is `web-time`.
- The pre-existing `device-snapshot-native.json` at
  `target/diagnostics/device-snapshot-native.json` (mtime 20:14:28,
  before this dispatch began) implies a prior dispatch successfully
  produced it before the `web_time` line was added/changed in
  `Cargo.toml`. The orchestrator's "fix" appears to have introduced
  the broken dependency declaration rather than corrected it.
- `cargo check` did not get far enough to surface any other warnings or
  errors; the registry-resolution failure is the only signal in the log.
- No `error[E…]` rustc diagnostic codes were emitted — this is a
  cargo-side dependency-resolution failure, not a source-code compile
  error.
