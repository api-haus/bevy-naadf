import * as fs from "node:fs/promises";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { test, expect, type ConsoleMessage } from "@playwright/test";
import { ConsoleCollector } from "./helpers/console-collector.js";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

/**
 * 2026-05-19 — `wasm-chunk-aadf-determinism` diagnostic. Boots the WASM
 * build in headed Chrome, waits for the `DeviceSnapshotPlugin` to emit its
 * one-shot `[device-snapshot] {...json...}` sentinel info line on
 * `console.log`, strips the prefix, writes the JSON to
 * `target/diagnostics/device-snapshot-web.json`.
 *
 * Pairs with `cargo run --bin e2e_render -- --device-snapshot-native`
 * (which writes `target/diagnostics/device-snapshot-native.json` directly
 * via `std::fs::write`). The `just diag-compare` recipe / `cargo run
 * --bin diag_compare` diffs the two.
 *
 * Headed Chrome only — per the project's e2e discipline headless WebGPU
 * dies with DeviceLost; see `justfile` block-comment "Why headed".
 *
 * No fixture is loaded — the snapshot is device-shape, not scene-shape.
 * The page just navigates to `/?ui=hide` and waits for the snapshot line.
 */

const REPO_ROOT = path.resolve(__dirname, "..", "..");
const DIAG_DIR = path.join(REPO_ROOT, "target", "diagnostics");
const WEB_SNAPSHOT_PATH = path.join(DIAG_DIR, "device-snapshot-web.json");
const SENTINEL = "[device-snapshot]";
// Closing sentinel — Bevy's wasm32 info!() formatter appends trailing
// tracing metadata after the message body, which broke JSON.parse when we
// captured "everything after the start sentinel." Now we extract the
// substring strictly between the two markers.
const END_SENTINEL = "[device-snapshot-end]";

test.describe("WASM device snapshot capture", () => {
  test.use({
    viewport: { width: 1280, height: 720 },
  });

  test("capture device-snapshot sentinel and write JSON to disk", async ({
    browser,
  }) => {
    test.setTimeout(180_000);
    const context = await browser.newContext({
      viewport: { width: 1280, height: 720 },
    });
    const page = await context.newPage();
    const collector = new ConsoleCollector();
    collector.attach(page);

    let snapshotLine: string | null = null;
    page.on("console", (msg: ConsoleMessage) => {
      const text = msg.text();
      const startIdx = text.indexOf(SENTINEL);
      if (startIdx < 0 || snapshotLine !== null) return;
      // Find the matching closing sentinel AFTER the start sentinel. The
      // JSON body lives between them. Bevy's wasm32 `info!` appends
      // tracing metadata after the message body, so "substring to
      // end-of-line" yields JSON + ~80 bytes of formatter junk.
      const afterStart = startIdx + SENTINEL.length;
      const endIdx = text.indexOf(END_SENTINEL, afterStart);
      if (endIdx < 0) {
        // Start sentinel seen, end sentinel not yet — wait for next msg.
        return;
      }
      snapshotLine = text.substring(afterStart, endIdx).trim();
      // eslint-disable-next-line no-console
      console.log(
        `[device-snapshot.spec] captured snapshot line (${snapshotLine.length} bytes)`,
      );
    });

    await page.goto("/?ui=hide", { waitUntil: "commit" });

    // The snapshot fires on the first render frame the RenderApp has
    // populated RenderAdapter/RenderDevice. Wait up to 120 s for the
    // sentinel line.
    await expect
      .poll(() => snapshotLine !== null, {
        timeout: 120_000,
        intervals: [1000, 2000],
        message:
          "waiting for `[device-snapshot] {json}` console line from " +
          "the WASM build (DeviceSnapshotPlugin fires on first RenderApp frame)",
      })
      .toBe(true);

    expect(snapshotLine, "snapshot line missing").not.toBeNull();
    const json = snapshotLine!;

    // Sanity check: the line should parse as JSON and have target = "web".
    let parsed: Record<string, unknown>;
    try {
      parsed = JSON.parse(json) as Record<string, unknown>;
    } catch (e) {
      throw new Error(
        `device-snapshot line is not valid JSON: ${
          (e as Error).message
        }\nline: ${json.slice(0, 500)}`,
      );
    }
    expect(parsed.target, "expected target='web' in snapshot").toBe("web");

    await fs.mkdir(DIAG_DIR, { recursive: true });
    await fs.writeFile(WEB_SNAPSHOT_PATH, json + "\n");
    // eslint-disable-next-line no-console
    console.log(
      `[device-snapshot.spec] wrote ${WEB_SNAPSHOT_PATH} (${json.length} bytes)`,
    );

    test.info().annotations.push({
      type: "device-snapshot-bytes",
      description: String(json.length),
    });

    await context.close();
  });
});
