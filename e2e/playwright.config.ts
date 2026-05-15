import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
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
        launchOptions: {
          // SwiftShader for headless WebGL in CI
          args: [
            "--use-gl=angle",
            "--use-angle=swiftshader",
            "--enable-unsafe-webgpu",
          ],
        },
      },
    },
  ],

  webServer: {
    command: "node serve.mjs",
    port: 4173,
    reuseExistingServer: !process.env.CI,
    // Give the server time to stat the WASM file on slow CI
    timeout: 10_000,
  },
});
