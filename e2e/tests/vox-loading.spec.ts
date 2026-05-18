import * as fs from "node:fs/promises";
import * as path from "node:path";
import { test, expect, type ConsoleMessage } from "@playwright/test";
import { ConsoleCollector } from "./helpers/console-collector.js";

/**
 * End-to-end test for the web `.vox` loading pipeline added in
 * `feat/web-vox-streaming`. The web build of `bevy-naadf` boots with the
 * embedded default scene installed by `voxel::grid::setup_test_grid`, then
 * `voxel::web_vox::startup_fetch_default_vox` HTTP-fetches the default
 * `.vox` from the R2 bucket and `apply_pending_vox` swaps it in over a
 * two-frame deferred-parse dance.
 *
 * This spec verifies the full lifecycle reaches the install-complete state:
 *
 *  1. The wasm binary downloads, compiles, and boots
 *     (`#loading.hidden` after `TrunkApplicationStarted`).
 *  2. The `.vox` fetch starts (`#loading` re-shown by web_vox).
 *  3. The bytes land and `install_vox_bytes_in_fixed_world` runs to
 *     completion — detected via the INFO log line
 *     "NAADF .vox loaded from {source} → ModelData (...)" emitted at
 *     `voxel::grid::install_vox_bytes_in_fixed_world`.
 *  4. No console errors, no Bevy ERROR-level logs, no wasm panics
 *     occurred at any point.
 *
 *  The post-install renderer state is captured to a PNG so on-failure we
 *  can tell "DeviceLost killed everything" (black canvas) apart from
 *  "scene rendered, then something else broke" (visible content).
 *
 *  The current failure mode tracked by the team is the wgpu buffer-usage
 *  panic in `populate_cpu_mirror_from_gpu_producer` — the
 *  `naadf_block_voxel_count_w2_placeholder` buffer lacks `CopySrc` on the
 *  wgpu WebGPU backend's stricter validation. That surfaces here as a Bevy
 *  ERROR log followed by a wasm panic, both of which the collector picks
 *  up — so this test will fail until the underlying buffer-flags fix
 *  lands. That's the intended signal.
 */
test.describe("Web .vox loading", () => {
  test("startup-fetches and installs the default .vox without errors", async ({
    page,
  }) => {
    const collector = new ConsoleCollector();
    collector.attach(page);

    // Watch for the install-complete log line emitted by
    // `install_vox_bytes_in_fixed_world`. The tracing-wasm bridge emits it
    // as `console.log` with `%cINFO%c` CSS markers; the literal message
    // text is still present.
    let voxInstallSeen = false;
    page.on("console", (msg: ConsoleMessage) => {
      if (msg.text().includes("NAADF .vox loaded from")) {
        voxInstallSeen = true;
      }
    });

    // Use the `?vox=<url>` query-string override (parsed by
    // `voxel::web_vox::resolve_startup_vox_url`) to point at a same-origin
    // copy of the Oasis fixture served by `serve.mjs` under
    // `/test-fixtures/`. The default URL targets the live R2 bucket which
    // may or may not have the right key uploaded — the test should not
    // depend on the live deploy.
    //
    // `waitUntil: "commit"` so we don't wait for the full WASM init
    // (Trunk's loader awaits `init()` which blocks the `load` event).
    await page.goto("/?vox=/test-fixtures/oasis_hard_cover.vox", {
      waitUntil: "commit",
    });

    // Phase 1: WASM init complete. The loader hides `#loading` once
    // `TrunkApplicationStarted` fires; then `web_vox` re-shows it with
    // "Downloading default model…". So we check for the *first*
    // hidden→shown transition is racy; instead we just wait for the
    // initial boot to finish (i.e. for `#loading.hidden` to ever become
    // attached at all).
    const loadingHidden = page.locator("#loading.hidden");
    await expect(loadingHidden).toBeAttached({ timeout: 90_000 });

    // Phase 2: The canvas must be visible — proves the renderer is alive
    // and `setup_test_grid` installed the default embedded scene before
    // the .vox fetch even starts. If this fails, boot itself is broken
    // and the smoke test catches it; we re-check here so the failure
    // message is local to .vox loading.
    const canvas = page.locator("canvas#bevy");
    await expect(canvas).toBeVisible({ timeout: 10_000 });

    // Phase 3: Wait for one of three terminal conditions:
    //   (a) the install-complete INFO log fires (happy path),
    //   (b) a wasm panic fires,
    //   (c) any console.error / Bevy ERROR / pageerror lands —
    //       including `DeviceLost`, which the headless WebGPU stack throws
    //       when the wasm main thread blocks too long during the sync
    //       parse and the browser's watchdog gives up on the device.
    //
    // Polling for (c) too means we bail out fast when something has clearly
    // gone wrong rather than burning the full 120 s budget on a generic
    // "expect didn't become truthy" timeout — and the final assertions
    // then surface the exact error text from `collector.errors`.
    //
    // Total budget is generous because the default .vox is ~85 MB (network)
    // + several seconds of synchronous parse on the wasm main thread (the
    // followup is to move that off-thread via `wasm-bindgen-rayon`; for now
    // we just give it room).
    await expect
      .poll(
        () =>
          collector.hasPanic ||
          collector.errors.length > 0 ||
          voxInstallSeen,
        {
          timeout: 120_000,
          message:
            "waiting for terminal condition: install-complete INFO log " +
            '("NAADF .vox loaded from …"), wasm panic, or any error',
        },
      )
      .toBeTruthy();

    if (collector.hasPanic) {
      // Fall through to the screenshot block below so the on-failure
      // image is still attached, then throw at the assertions stage with
      // a more useful message than `expect.poll` produces on its own.
      // We don't `test.fail()` here — that's for marking expected
      // failures; this is a real failure we want surfaced.
    }

    // Phase 4: Give the renderer time to actually draw the post-install
    // frames. The two-stage deferred-parse dance in `apply_pending_vox`
    // means the install runs on frame N+1 after the bytes arrive; the GPU
    // producer chain then has to dispatch its compute passes (W5) and the
    // CPU-mirror readback has to round-trip. A 10 s wait covers all of
    // that even on slow GPUs.
    await page.waitForTimeout(10_000);

    // Phase 5: Snapshot the canvas. Attached to the Playwright HTML
    // report AND written to the test-results dir so it's accessible
    // without opening the report. We do this regardless of pass/fail so a
    // black canvas on failure can be told apart from "scene rendered,
    // then something downstream broke".
    try {
      const png = await canvas.screenshot();
      await test.info().attach("canvas-after-vox-install", {
        body: png,
        contentType: "image/png",
      });
      await fs.writeFile(
        path.join(test.info().outputDir, "canvas-after-vox-install.png"),
        png,
      );
    } catch (err) {
      test.info().annotations.push({
        type: "screenshot-failed",
        description: String(err),
      });
    }

    // Phase 6: Report all collected errors as annotations so the HTML
    // report shows them inline against this test.
    for (const err of collector.errors) {
      test.info().annotations.push({
        type: err.type,
        description: err.text,
      });
    }

    // Final assertions.
    expect(
      collector.hasPanic,
      `WASM panic during .vox lifecycle: ${collector.firstPanic}`,
    ).toBe(false);

    expect(
      collector.errors,
      `Console/Bevy errors during .vox lifecycle:\n` +
        collector.errors
          .map((e) => `  [${e.type}] ${e.text.slice(0, 500)}`)
          .join("\n"),
    ).toHaveLength(0);

    expect(
      voxInstallSeen,
      "Expected to see the install-complete INFO log " +
        '("NAADF .vox loaded from …") within the timeout',
    ).toBe(true);
  });
});
