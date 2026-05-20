# Playwright stale-server footgun fix

## Approach chosen
B — pretest kill-stale-listeners via `globalSetup`.

## Rationale (one paragraph)
Approach A (`reuseExistingServer: false`) would spawn a fresh `node serve.mjs`
on every invocation, which means a full cold-start of the static server (plus
`lsof`/port-probe handshake) even when the developer is iterating through
multiple test files in a single session. That's a minor ergonomic cost but
cumulative across an SSIM sweep of 10 web runs. Approach B preserves
`reuseExistingServer: !process.env.CI` — so multiple test files in one
session continue sharing a single warm server — but adds a `globalSetup`
hook that evicts any stale listener on `:4173` before Playwright's `webServer`
block runs. The eviction is a single `lsof -ti :4173 | xargs -r kill -9` call:
a no-op when the port is free (xargs `-r` / `--no-run-if-empty`) and a clean
kill when a squatter is present. The script catches and logs any error rather
than hard-failing the suite, so if `lsof` were somehow missing (it isn't —
confirmed below), Playwright would still produce an intelligible error about
the occupied port rather than silently hanging. Approach B is strictly better
for a worktree-heavy workflow: it eliminates the footgun while not
regressing multi-file test runs.

## Files changed
| File | Lines | Change |
|---|---|---|
| `e2e/playwright.config.ts` | +5 | Add `globalSetup: "./kill-stale-server.mjs"` with explanatory comment |
| `e2e/kill-stale-server.mjs` | +38 (new file) | `globalSetup` module: `lsof -ti :4173 | xargs -r kill -9`, error-tolerant |

## Diffs (unified)

### `e2e/playwright.config.ts`

```diff
--- a/e2e/playwright.config.ts
+++ b/e2e/playwright.config.ts
@@ -2,6 +2,11 @@ import { defineConfig, devices } from "@playwright/test";
 
 export default defineConfig({
   testDir: "./tests",
+  // Evict any stale `node serve.mjs` from a deleted worktree before
+  // Playwright's webServer block runs.  Without this, reuseExistingServer
+  // silently picks up a squatter that 404s every request.  See
+  // docs/orchestrate/wasm-chunk-aadf-nondeterminism/15-playwright-stale-server-fix.md
+  globalSetup: "./kill-stale-server.mjs",
   fullyParallel: false,
   forbidOnly: !!process.env.CI,
   retries: 0,
```

### `e2e/kill-stale-server.mjs` (new file)

```diff
--- /dev/null
+++ b/e2e/kill-stale-server.mjs
@@ -0,0 +1,38 @@
+// Playwright globalSetup — kills any stale process squatting on port 4173
+// before the test run begins.
+//
+// Background: `reuseExistingServer: !process.env.CI` means local runs
+// silently reuse whatever is listening on 4173 — including stale `node
+// serve.mjs` processes left over from a deleted worktree whose `dist/`
+// directory no longer exists. Those squatters 404 every request but
+// Playwright can't tell the difference from a healthy server at startup,
+// so the entire suite dies with confusing failures rather than a clear
+// "server not found" error.  The fix: evict any squatter unconditionally
+// before Playwright's webServer block runs.
+//
+// Linux-only (project requirement per CLAUDE.md environment notes).
+// The `lsof -ti :PORT` pattern prints the PID(s) of all listeners, then
+// `xargs -r kill -9` kills them. `-r` (--no-run-if-empty) makes xargs a
+// no-op when the port is free, so this is safe to run on a clean machine.
+
+import { execSync } from "node:child_process";
+
+const PORT = 4173;
+
+export default async function killStaleServer() {
+  try {
+    execSync(`lsof -ti :${PORT} | xargs -r kill -9`, { stdio: "pipe" });
+    console.log(`[globalSetup] Evicted stale listener(s) on :${PORT} (if any).`);
+  } catch {
+    // If lsof finds nothing, xargs -r exits 0.  Any other error (e.g.,
+    // lsof not installed) is non-fatal: if the port is actually free the
+    // suite will work anyway; if it's occupied the webServer startup will
+    // surface a proper error message.
+    console.warn(
+      `[globalSetup] Warning: could not kill stale server on :${PORT}. ` +
+        "Continuing — Playwright will fail with a clear error if the port is occupied."
+    );
+  }
+}
```

## Verification

- **Syntax check — `playwright.config.ts`:** Reviewed the final file state
  manually. The added `globalSetup` property is a valid Playwright
  `defineConfig` field (type `string | (() => Promise<unknown>)`). The rest
  of the file is structurally unchanged. No new imports needed — `globalSetup`
  accepts a path string. The config is valid TypeScript.
- **Syntax check — `kill-stale-server.mjs`:** Pure ES module with a default
  async export. `execSync` is from `node:child_process` (Node builtin). No
  TypeScript-specific syntax used — it's `.mjs` not `.ts`, so no compilation
  step needed. Reviewed for structural correctness: try/catch, const, template
  literal — all standard.
- **`lsof -ti :4173` sanity check:** Run in the current environment before
  editing. `lsof -ti :4173` returned exit code 1 with no output — confirms
  `lsof` is available on the system and returns cleanly when no listener is
  present. `xargs -r` (`--no-run-if-empty`) prevents `kill -9` from receiving
  no arguments and exiting non-zero. Availability confirmed.

## Side notes / observations / complaints (MANDATORY per CLAUDE.md)

### Footguns in `playwright.config.ts`

1. **`reuseExistingServer: !process.env.CI` is the root footgun this fix
   addresses.** The semantics are "if not CI, silently reuse whatever is on
   the port" — which is a useful optimisation for edit-test loops but fatal
   for worktree-rotation workflows where a stale process can linger. The
   `globalSetup` fix makes this safe. If a future operator changes
   `reuseExistingServer` unconditionally to `true`, the `globalSetup` script
   ensures it's still a fresh server each run.

2. **`timeout: 10_000` on `webServer` is tight for the first run of a cold
   `cargo build --release` pipeline.**  In practice the caller always pre-
   builds before running the e2e suite, so the serve.mjs starts in <100 ms.
   But if someone runs `npx playwright test` on a freshly checked-out repo
   without pre-building, the `command: "node serve.mjs"` will start in 10 ms
   (the server itself is fast) but the WASM at `dist/` won't exist — leading
   to 404s that look like the server misbehaved, not a missing build artifact.
   Worth a comment (or a pre-flight `distDir` existence check inside
   `serve.mjs`) but out of scope here.

3. **`workers: 1` + `fullyParallel: false` + `maxFailures: 1` is the correct
   belt-and-suspenders for a stateful GPU test suite.** No issue, just noting
   these are intentional constraints, not defaults — future contributors
   should not "optimise" them away without understanding the WebGPU
   serialisation requirement.

4. **No `globalTeardown` is wired.** The `globalSetup` script only kills
   squatters at the start of a run; it doesn't kill the server Playwright
   spawns at the end. That's correct: Playwright's `webServer` lifecycle
   management handles teardown of the server it spawned. Only orphaned
   squatters (i.e., servers Playwright did NOT spawn in the current run) need
   the global-setup kill. This is working as intended.

### Subjective reactions

The fix is appropriately minimal given the brief. Approach B adds ~40 lines
across two files to eliminate a class of ~15-minute debugging sessions that
will recur on every worktree-deletion cycle. The value-to-cost ratio is
strongly positive.

The `ls` tool was non-functional in this environment (rtk filter returning
empty for all paths, including confirmed-populated directories). Fell back to
`find -maxdepth 1` for directory introspection. The fix itself was unaffected
— `Read` and `Write` tools operate on absolute paths and worked correctly.

The `serve.mjs` examination confirmed the server has no mutable state or
cleanup artifacts: it's a pure stateless static-file server using
`http.createServer` + `createReadStream`. Killing it with `SIGKILL` (which
`kill -9` sends) leaves no orphaned resources. A graceful `SIGTERM` would be
cleaner but the footgun scenario by definition involves a process that isn't
responding to lifecycle signals from its parent (the parent Playwright runner
is gone) — `kill -9` is the correct choice.
