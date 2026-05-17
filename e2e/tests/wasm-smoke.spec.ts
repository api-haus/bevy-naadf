import * as fs from "node:fs/promises";
import * as path from "node:path";
import { test, expect } from "@playwright/test";
import { ConsoleCollector } from "./helpers/console-collector.js";

test.describe("WASM Smoke Test", () => {
  test("loads without panics and renders the bevy canvas", async ({ page }) => {
    const collector = new ConsoleCollector();
    collector.attach(page);

    // Phase 1: Navigate with "commit" so we don't wait for the full WASM init.
    // Trunk's module script does `await init(wasm)` which blocks the load event,
    // so the default "load" waitUntil would skip past the loading screen.
    await page.goto("/", { waitUntil: "commit" });

    // Phase 2: Wait for the loading overlay to hide.
    // This proves the full WASM init chain completed:
    //   download -> compile -> wasm_main() -> App::run() -> TrunkApplicationStarted
    // #loading starts visible and gets .hidden when TrunkApplicationStarted fires.
    const loading = page.locator("#loading.hidden");
    await expect(loading).toBeAttached({ timeout: 90_000 });

    // Fail fast if a panic already occurred during init
    if (collector.hasPanic) {
      test.fail(
        true,
        `WASM panic during initialization: ${collector.firstPanic}`
      );
      return;
    }

    // Phase 3: Verify the Bevy canvas is visible (rendering started)
    const canvas = page.locator("canvas#bevy");
    await expect(canvas).toBeVisible({ timeout: 10_000 });

    // Phase 4: Wait for runtime systems to execute
    // Several compute pipelines compile lazily (e.g. naadf_map_copy_pipeline
    // hits CreateComputePipeline late in the boot sequence). 10 s gives the
    // post-boot pipeline-init cascade time to fire — too-short waits miss
    // validation errors that surface after the device is destroyed by an
    // earlier failure.
    await page.waitForTimeout(10_000);

    // Phase 4.5: Snapshot the canvas regardless of pass/fail outcome.
    // - On a pass run, the PNG is the visual confirmation the renderer
    //   reached the framebuffer.
    // - On a fail run, the PNG distinguishes "DeviceLost killed everything"
    //   (black canvas) from "some passes ran" (partial content).
    // Attached to the Playwright HTML report AND written to test-results/
    // so it's accessible without opening the report.
    try {
      const png = await page.locator("canvas#bevy").screenshot();
      await test.info().attach("canvas-after-10s", {
        body: png,
        contentType: "image/png",
      });
      await fs.writeFile(
        path.join(test.info().outputDir, "canvas-after-10s.png"),
        png,
      );
    } catch (err) {
      // Don't let the screenshot failure mask the real error — log it as
      // an annotation and let the error assertions below decide pass/fail.
      test.info().annotations.push({
        type: "screenshot-failed",
        description: String(err),
      });
    }

    // Report all collected errors in test output for debugging
    for (const err of collector.errors) {
      test.info().annotations.push({
        type: err.type,
        description: err.text,
      });
    }

    // Final assertions
    expect(
      collector.hasPanic,
      `WASM panic detected: ${collector.firstPanic}`
    ).toBe(false);

    expect(
      collector.errors,
      `Console errors detected:\n${collector.errors.map((e) => `  [${e.type}] ${e.text}`).join("\n")}`
    ).toHaveLength(0);
  });
});
