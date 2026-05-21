import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  // Stale-server footgun: a `node serve.mjs` left from a deleted worktree
  // can squat on :4173. The earlier fix (globalSetup: kill-stale-server.mjs)
  // didn't work because Playwright runs globalSetup AFTER webServer is
  // started — the hook killed the just-spawned serve.mjs, breaking every
  // run. Switched to `reuseExistingServer: false` below: Playwright always
  // spawns a fresh server and manages its lifecycle, so the footgun only
  // surfaces if the user manually started serve.mjs (in which case
  // Playwright fails with a clear "port in use" error rather than silently
  // 404'ing). Original analysis: docs/orchestrate/wasm-chunk-aadf-nondeterminism/15-playwright-stale-server-fix.md
  // — note that doc's lifecycle-order claim is wrong; this comment carries
  // the correction.
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  retries: 0,
  workers: 1,
  maxFailures: 1,

  // WASM load + compile + thread pool init + asset loading is slow
  timeout: 120_000,
  expect: { timeout: 90_000 },

  reporter: process.env.CI ? "github" : "list",

  use: {
    baseURL: "http://localhost:4173",
    trace: "retain-on-failure",
    video: "retain-on-failure",
  },

  projects: [
    {
      name: "chromium",
      use: {
        ...devices["Desktop Chrome"],
        // The NAADF render path is WebGPU-only. Playwright's bundled chromium
        // defaults to `chrome-headless-shell` (the old lightweight headless
        // implementation) — its WebGPU stack is the SwiftShader-only fallback
        // and dies with `DeviceLost: Destroyed` mid-render on our compute
        // pipeline. Switching to the system Chrome (`channel: "chrome"`)
        // routes through full headless-new mode, which has the same Dawn /
        // GPU process pipeline as headed Chrome and can pick the host
        // adapter (real GPU when available, fully-featured SwiftShader-
        // Vulkan when not).
        channel: "chrome",
        launchOptions: {
          // `--enable-unsafe-webgpu` is still required: WebGPU is gated to
          // secure contexts + this flag in Chrome stable. Developer-features
          // surface Dawn validation errors as page errors instead of
          // silently destroying the device. `--no-sandbox` is needed when
          // Playwright launches Chrome under a non-default user namespace.
          args: [
            "--no-sandbox",
            "--enable-unsafe-webgpu",
            "--enable-webgpu-developer-features",
          ],
        },
      },
    },
  ],

  webServer: {
    command: "node serve.mjs",
    port: 4173,
    reuseExistingServer: false,
    // Give the server time to stat the WASM file on slow CI
    timeout: 10_000,
  },
});
