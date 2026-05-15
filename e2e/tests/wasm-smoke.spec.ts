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
    // Some panics only trigger on frame 2+ (e.g., web_time in Update schedule)
    await page.waitForTimeout(5_000);

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
