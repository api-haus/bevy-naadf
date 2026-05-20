// Playwright globalSetup — kills any stale process squatting on port 4173
// before the test run begins.
//
// Background: `reuseExistingServer: !process.env.CI` means local runs
// silently reuse whatever is listening on 4173 — including stale `node
// serve.mjs` processes left over from a deleted worktree whose `dist/`
// directory no longer exists. Those squatters 404 every request but
// Playwright can't tell the difference from a healthy server at startup,
// so the entire suite dies with confusing failures rather than a clear
// "server not found" error.  The fix: evict any squatter unconditionally
// before Playwright's webServer block runs.
//
// Linux-only (project requirement per CLAUDE.md environment notes).
// The `lsof -ti :PORT` pattern prints the PID(s) of all listeners, then
// `xargs -r kill -9` kills them. `-r` (--no-run-if-empty) makes xargs a
// no-op when the port is free, so this is safe to run on a clean machine.

import { execSync } from "node:child_process";

const PORT = 4173;

export default async function killStaleServer() {
  try {
    execSync(`lsof -ti :${PORT} | xargs -r kill -9`, { stdio: "pipe" });
    console.log(`[globalSetup] Evicted stale listener(s) on :${PORT} (if any).`);
  } catch {
    // If lsof finds nothing, xargs -r exits 0.  Any other error (e.g.,
    // lsof not installed) is non-fatal: if the port is actually free the
    // suite will work anyway; if it's occupied the webServer startup will
    // surface a proper error message.
    console.warn(
      `[globalSetup] Warning: could not kill stale server on :${PORT}. ` +
        "Continuing — Playwright will fail with a clear error if the port is occupied."
    );
  }
}
