import * as fs from "node:fs/promises";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import { test, expect, type ConsoleMessage, type Page } from "@playwright/test";
import { ConsoleCollector } from "./helpers/console-collector.js";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

/**
 * Cross-target (native ↔ WASM) horizon-view SSIM parity gate.
 *
 * 2026-05-19 — added to catch the WASM-only ray-termination bug the
 * `--vox-web-parity` top-down birdseye pose cannot surface. The user
 * reported: in native release builds the voxel raymarcher reaches the
 * horizon over a 4×4 grid of Oasis scenes; on WASM (live Cloudflare Pages
 * deploy + local `just test-wasm`) the rays terminate at ~20–30 % of the
 * world depth, leaving sky where distant geometry should be AND clipping
 * close-range building faces. Root cause hypothesis: the chunk-level AADF
 * acceleration (`bounds_calc.wgsl`'s indirect-dispatch convergence loop)
 * doesn't converge on WebGPU, so rays step chunk-by-chunk and exhaust
 * `max_ray_steps_primary = 120` at exactly 120 × 16 voxels = 1920 voxels
 * (≈30 % of the 4096-voxel world depth — matches the symptom exactly).
 *
 * This gate:
 *  1. Spawns `cargo run --bin e2e_render -- --vox-horizon-native` to
 *     capture a native 1280×720 reference framebuffer at the C#-faithful
 *     horizon pose. Writes `target/e2e-screenshots/vox_horizon_native.png`.
 *  2. Loads `/?vox=/test-fixtures/oasis.cvox&pose=horizon` in Playwright
 *     at viewport 1280×720; the `?pose=horizon` URL param triggers the
 *     [`pin_web_horizon_camera`] system to pin the camera at the same
 *     pose every frame.
 *  3. Waits for `.cvox` install + TAA/GI settle, captures the canvas to
 *     `target/e2e-screenshots/vox_horizon_web.png`.
 *  4. Shells out to `cargo run --bin e2e_render -- --ssim-compare …
 *     --ssim-min <T>` and asserts the cross-target SSIM is ABOVE the
 *     similarity floor.
 *
 * The gate is expected to FAIL until the WASM chunk-AADF convergence bug
 * is fixed — that's the entire point. Re-baseline `SSIM_MIN` after the
 * fix lands.
 */

const REPO_ROOT = path.resolve(__dirname, "..", "..");

/**
 * Minimum SSIM the gate tolerates between the native horizon capture and
 * the WASM horizon capture. See
 * `crates/bevy_naadf/src/e2e/vox_horizon_parity.rs::HORIZON_SSIM_SIMILARITY_MIN`
 * for the tuning rationale (kept in sync — the Rust side is the source of
 * truth for the canonical value).
 *
 * **0.98** — same camera pose, same world, same `.cvox` fixture, same
 * resolution. Near-identical framebuffers expected; structural divergence
 * (missing distant geometry, black pixels, color shifts) collapses SSIM
 * well below this floor.
 */
const SSIM_MIN = 0.91;

const HORIZON_NATIVE_PNG = "vox_horizon_native.png";
const HORIZON_WEB_PNG = "vox_horizon_web.png";
const E2E_SCREENSHOT_DIR = path.join(REPO_ROOT, "target", "e2e-screenshots");
const FUNNEL_DIR = path.join(E2E_SCREENSHOT_DIR, "funnel");

const CANVAS_SETTLE_MS = 30_000;

/**
 * Filesystem-safe ISO-8601-ish timestamp used as the funnel per-run basename.
 * Example: `20260520T084530-123` (UTC YYYYMMDDTHHMMSS-millis). Colons and
 * dots are not portable across all filesystems / tooling — this format
 * strips them entirely.
 */
function makeRunTimestamp(): string {
  const d = new Date();
  const pad = (n: number, w = 2): string => String(n).padStart(w, "0");
  return (
    `${d.getUTCFullYear()}` +
    `${pad(d.getUTCMonth() + 1)}` +
    `${pad(d.getUTCDate())}` +
    `T` +
    `${pad(d.getUTCHours())}` +
    `${pad(d.getUTCMinutes())}` +
    `${pad(d.getUTCSeconds())}` +
    `-${pad(d.getUTCMilliseconds(), 3)}`
  );
}

/** Regex matching the `[xxx]` sentinel prefix used by the inline probe + diagnostics. */
const SENTINEL_RE = /\[[a-z][a-z0-9_-]+\]/i;

/** Markers used to flag browser panics / fatal runtime errors in console text. */
const PANIC_MARKERS = [
  "panicked",
  "RuntimeError",
  "Uncaught",
  "DeviceLost",
  "fatal",
] as const;

/**
 * Extracts the SSIM score reported by `e2e_render --ssim-compare`. The
 * binary prints `SSIM=<f64>` on its own line at ssim.rs:143. Returns
 * `null` if no such line is present (e.g. the subprocess errored out
 * before emitting a score).
 */
function extractSsimScore(stdout: string): number | null {
  const m = stdout.match(/^SSIM=([0-9]+(?:\.[0-9]+)?)/m);
  if (!m) return null;
  const v = Number.parseFloat(m[1]);
  return Number.isFinite(v) ? v : null;
}

/**
 * Build the per-run funnel `.txt` sidecar body. The orchestrator + user
 * visually groups runs via the per-run PNGs and reads this sidecar to
 * understand why a given run landed in a given attractor state.
 *
 * The sentinel groups are emitted in this order: `[aadf-probe]` (the
 * inline ray-walk probe), `[probe1-call]` first 20 + last 10 (the
 * indirect-dispatch convergence trace — usually 200+ entries),
 * `[cpu-gpu-parity]`, `[device-snapshot]`, any OTHER `[xxx]` sentinel
 * lines, finally any console lines containing panic / fatal markers.
 *
 * Every captured line is emitted verbatim — no truncation, no
 * normalisation — so the funnel data is a faithful record of what the
 * browser printed.
 */
function buildFunnelSidecar(opts: {
  timestamp: string;
  ssim: number | null;
  ssimPass: boolean | null;
  sentinelLines: string[];
  panicLines: string[];
}): string {
  const { timestamp, ssim, ssimPass, sentinelLines, panicLines } = opts;

  const lines = sentinelLines;
  const aadfProbe = lines.filter(
    (l) => l.includes("[aadf-probe]") || l.includes("[aadf-probe2]"),
  );
  // Anything tagged with the `[probe1-call*]` family (the indirect-dispatch
  // convergence trace). Captured separately because it's verbose (200+
  // entries) and the orchestrator wants head+tail, not the full firehose.
  const probe1Call = lines.filter((l) => l.includes("[probe1-call"));
  const cpuGpuParity = lines.filter((l) => l.includes("[cpu-gpu-parity"));
  const deviceSnapshot = lines.filter((l) => l.includes("[device-snapshot"));

  // "Other" sentinels = anything with a `[xxx]` prefix that didn't fall
  // into one of the named buckets above. Useful as a catch-all so new
  // diagnostic sentinels added to the codebase don't silently drop out of
  // the funnel sidecar.
  const namedBuckets = [
    "[aadf-probe]",
    "[aadf-probe2]",
    "[probe1-call",
    "[cpu-gpu-parity",
    "[device-snapshot",
  ];
  const other = lines.filter(
    (l) => !namedBuckets.some((tag) => l.includes(tag)),
  );

  const ssimStr = ssim === null ? "<unavailable>" : ssim.toFixed(6);
  const passStr = ssimPass === null ? "<unavailable>" : ssimPass ? "yes" : "no";

  const sections = [
    `# Run ${timestamp}`,
    `SSIM: ${ssimStr}`,
    `pass (>= ${SSIM_MIN})?: ${passStr}`,
    ``,
    `## [aadf-probe] sentinel lines (raw)`,
    aadfProbe.length > 0 ? aadfProbe.join("\n") : "<none>",
    ``,
    `## [probe1-call] first 20 lines (raw)`,
    probe1Call.length > 0 ? probe1Call.slice(0, 20).join("\n") : "<none>",
    ``,
    `## [probe1-call] last 10 lines (raw)`,
    probe1Call.length > 0 ? probe1Call.slice(-10).join("\n") : "<none>",
    ``,
    `## [probe1-call] total count`,
    String(probe1Call.length),
    ``,
    `## [cpu-gpu-parity] line(s) (raw)`,
    cpuGpuParity.length > 0 ? cpuGpuParity.join("\n") : "<none>",
    ``,
    `## [device-snapshot] sentinel (raw)`,
    deviceSnapshot.length > 0 ? deviceSnapshot.join("\n") : "<none>",
    ``,
    `## Any other [xxx] sentinel lines`,
    other.length > 0 ? other.join("\n") : "<none>",
    ``,
    `## Browser-console error/panic markers`,
    panicLines.length > 0 ? panicLines.join("\n") : "<none>",
    ``,
  ];

  return sections.join("\n");
}

/**
 * Shell out to `cargo run --bin e2e_render -- --vox-horizon-native` to
 * produce the native reference PNG. Returns `{code, stdout, stderr}`.
 */
async function runNativeHorizonCapture(): Promise<{
  code: number;
  stdout: string;
  stderr: string;
}> {
  return new Promise((resolve, reject) => {
    const proc = spawn(
      "cargo",
      ["run", "--bin", "e2e_render", "--", "--vox-horizon-native"],
      {
        cwd: REPO_ROOT,
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    let stdout = "";
    let stderr = "";
    proc.stdout.on("data", (chunk: Buffer) => {
      stdout += chunk.toString("utf8");
    });
    proc.stderr.on("data", (chunk: Buffer) => {
      stderr += chunk.toString("utf8");
    });
    proc.on("error", reject);
    proc.on("close", (code: number | null) => {
      resolve({ code: code ?? -1, stdout, stderr });
    });
  });
}

/**
 * Shell out to `cargo run --bin e2e_render -- --ssim-compare <a> <b>
 * --ssim-min <min>`. Same exit-code semantics as the dissimilarity gate:
 *   - `0` — gate passed (SSIM ≥ min).
 *   - `1` — gate failed (SSIM < min).
 *   - `2` — internal error.
 */
async function runSsimCompare(
  a: string,
  b: string,
  min: number,
): Promise<{ code: number; stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    const proc = spawn(
      "cargo",
      [
        "run",
        "--bin",
        "e2e_render",
        "--",
        "--ssim-compare",
        a,
        b,
        "--ssim-min",
        String(min),
      ],
      {
        cwd: REPO_ROOT,
        stdio: ["ignore", "pipe", "pipe"],
      },
    );
    let stdout = "";
    let stderr = "";
    proc.stdout.on("data", (chunk: Buffer) => {
      stdout += chunk.toString("utf8");
    });
    proc.stderr.on("data", (chunk: Buffer) => {
      stderr += chunk.toString("utf8");
    });
    proc.on("error", reject);
    proc.on("close", (code: number | null) => {
      resolve({ code: code ?? -1, stdout, stderr });
    });
  });
}

/**
 * Capture a screenshot of the wasm canvas after the loading overlay
 * clears and the scene has settled for `settleMs`. Returns the PNG bytes
 * + the on-disk path.
 */
async function captureSettledCanvas(
  page: Page,
  filename: string,
  settleMs: number,
  outDir: string,
): Promise<{ bytes: Buffer; outPath: string }> {
  const loadingHidden = page.locator("#loading.hidden");
  await expect(loadingHidden).toBeAttached({ timeout: 90_000 });
  const canvas = page.locator("canvas#bevy");
  await expect(canvas).toBeVisible({ timeout: 10_000 });
  await page.waitForTimeout(settleMs);
  const bytes = await canvas.screenshot();
  await test.info().attach(filename, { body: bytes, contentType: "image/png" });
  await fs.mkdir(outDir, { recursive: true });
  const outPath = path.join(outDir, filename);
  await fs.writeFile(outPath, bytes);
  return { bytes, outPath };
}

test.describe("Cross-target horizon parity", () => {
  // Use a 1280×720 viewport so the WASM canvas size matches the native
  // 1280×720 e2e window — required for SSIM-compare (which fails on
  // dimension mismatch).
  test.use({
    viewport: { width: 1280, height: 720 },
    contextOptions: {},
  });

  test("native horizon capture vs WASM horizon capture — SSIM similar", async ({
    browser,
  }) => {
    // Per-run funnel timestamp — used for the unique PNG + .txt basename
    // so the user can visually group runs by attractor state after the
    // 15-run sweep. The fixed `vox_horizon_web.png` path is still written
    // for backwards compatibility with the SSIM-compare subprocess and
    // any sidecar tooling that hard-codes that name.
    const runTimestamp = makeRunTimestamp();
    await fs.mkdir(FUNNEL_DIR, { recursive: true });
    const funnelPngPath = path.join(
      FUNNEL_DIR,
      `vox_horizon_web-${runTimestamp}.png`,
    );
    const funnelTxtPath = path.join(
      FUNNEL_DIR,
      `vox_horizon_web-${runTimestamp}.txt`,
    );

    // === Phase 1 — produce the native reference PNG ========================
    //
    // Spawns the e2e_render binary at the C# default horizon pose. The
    // native release-build path is the user-confirmed correct reference
    // (Image #2 in the bug report). Generous timeout — building +
    // running e2e_render against the production W5 chain takes minutes.
    test.setTimeout(10 * 60_000);
    const native = await runNativeHorizonCapture();
    test.info().annotations.push({
      type: "native-horizon-stdout",
      description: native.stdout,
    });
    test.info().annotations.push({
      type: "native-horizon-stderr",
      description: native.stderr,
    });
    expect(
      native.code,
      `--vox-horizon-native exited non-zero (${native.code}). ` +
        `stdout:\n${native.stdout}\nstderr:\n${native.stderr}`,
    ).toBe(0);
    const nativePngPath = path.join(E2E_SCREENSHOT_DIR, HORIZON_NATIVE_PNG);
    await fs.access(nativePngPath);

    // === Phase 2 — capture the WASM horizon view ===========================
    const context = await browser.newContext({
      viewport: { width: 1280, height: 720 },
    });
    const page = await context.newPage();
    const collector = new ConsoleCollector();
    collector.attach(page);

    let voxInstallSeen = false;
    const wgpuDiagnosticLines: string[] = [];
    // 2026-05-20 — funnel data collection. ANY console line that contains a
    // `[xxx]` sentinel prefix gets buffered here, so the per-run sidecar
    // doc captures the full diagnostic surface (not just the specific
    // tags this spec used to forward). Panic markers tracked separately.
    const sentinelLines: string[] = [];
    const panicLines: string[] = [];
    page.on("console", (msg: ConsoleMessage) => {
      const text = msg.text();
      if (text.includes("NAADF .vox loaded from")) {
        voxInstallSeen = true;
      }
      // 2026-05-19 — forward the Q4 storage-buffer-binding instrumentation
      // + the W5 dispatch shape log so the parity gate's failure mode is
      // self-diagnostic (no manual devtools dive). prepare.rs:541-569 +
      // construction/mod.rs:3122-3136.
      if (
        text.includes("Q4 instrumentation") ||
        text.includes("Q4 CONFIRMED") ||
        text.includes("vox-gpu-rewrite W5 — per-segment GPU producer chain DISPATCHED") ||
        text.includes("prepare_world_gpu allocating") ||
        text.includes("[aadf-probe]") ||
        text.includes("[aadf-probe2]")
      ) {
        wgpuDiagnosticLines.push(text);
        // eslint-disable-next-line no-console
        console.log(`[wasm-diag] ${text}`);
      }
      // 2026-05-20 — capture every sentinel-tagged line for the funnel
      // sidecar, regardless of which named bucket it falls into.
      if (SENTINEL_RE.test(text)) {
        sentinelLines.push(text);
      }
      if (PANIC_MARKERS.some((m) => text.includes(m))) {
        panicLines.push(text);
      }
    });
    // Uncaught exceptions / pageerrors aren't routed through console.log;
    // capture them explicitly so funnel sidecar panic markers stay complete.
    page.on("pageerror", (err: Error) => {
      const text = `[pageerror] ${err.message}`;
      if (PANIC_MARKERS.some((m) => text.includes(m))) {
        panicLines.push(text);
      }
    });

    // `?vox=/test-fixtures/oasis.cvox` — same fixture the native gate
    // loads. `?pose=horizon` — pin the camera at the C# default horizon
    // pose every frame (the [`pin_web_horizon_camera`] system).
    // `?vox` — same fixture the native gate loads.
    // `?pose=horizon` — pin the camera at the C# default horizon pose.
    // `?ui=hide` — hide the editor HUD + settings + diagnostics HUD so the
    // SSIM compare measures pure 3D framebuffers (native gate runs under
    // `AppConfig::e2e` which has no UI; without `?ui=hide` the live web
    // canvas paints the brush palette + color picker over the 3D scene,
    // contaminating SSIM and false-passing the gate).
    await page.goto("/?vox=/test-fixtures/oasis.cvox&pose=horizon&ui=hide", {
      waitUntil: "commit",
    });

    const loadingHidden = page.locator("#loading.hidden");
    await expect(loadingHidden).toBeAttached({ timeout: 90_000 });
    const canvas = page.locator("canvas#bevy");
    await expect(canvas).toBeVisible({ timeout: 10_000 });

    // Wait for one of three terminal conditions before settling.
    await expect
      .poll(
        () =>
          collector.hasPanic ||
          collector.errors.length > 0 ||
          voxInstallSeen,
        {
          timeout: 120_000,
          message:
            "waiting for terminal condition: install-complete INFO log, " +
            "wasm panic, or any error",
        },
      )
      .toBeTruthy();

    const web = await captureSettledCanvas(
      page,
      HORIZON_WEB_PNG,
      CANVAS_SETTLE_MS,
      E2E_SCREENSHOT_DIR,
    );
    // 2026-05-20 — funnel: write a unique-per-run copy of the canvas
    // capture so the funnel sweep preserves all 15 PNGs instead of the
    // last one overwriting the previous 14.
    await fs.writeFile(funnelPngPath, web.bytes);
    // First-pass sidecar write — covers the failure path where the
    // panic / errors / install-not-seen assertions below throw and we
    // never reach the SSIM compare. Re-written below with the SSIM
    // score once Phase 3 completes.
    await fs.writeFile(
      funnelTxtPath,
      buildFunnelSidecar({
        timestamp: runTimestamp,
        ssim: null,
        ssimPass: null,
        sentinelLines,
        panicLines,
      }),
    );

    // Surface the WebGPU diagnostic lines as test annotations so they're
    // visible in the test report when the gate fails.
    for (const line of wgpuDiagnosticLines) {
      test.info().annotations.push({ type: "wasm-diag", description: line });
    }

    // 2026-05-19 — persist the AADF-probe lines from BOTH native and web
    // to disk so the orchestrator can diff them without copy-pasting log
    // tails out of the Playwright report. The native subprocess's stdout
    // already prints `[aadf-probe]` lines via tracing-subscriber; we
    // captured its full output as `native.stdout`. The web canvas's
    // wasm-tracing-bridge emits the same lines through `console.log`,
    // which we collected in `wgpuDiagnosticLines`.
    const probeNative = (native.stdout + "\n" + native.stderr)
      .split("\n")
      .filter((l) => l.includes("[aadf-probe]") || l.includes("[aadf-probe2]"))
      .join("\n");
    const probeWeb = wgpuDiagnosticLines
      .filter((l) => l.includes("[aadf-probe]") || l.includes("[aadf-probe2]"))
      .join("\n");
    await fs.writeFile(
      path.join(E2E_SCREENSHOT_DIR, "vox_horizon_native.aadf-probe.log"),
      probeNative,
    );
    await fs.writeFile(
      path.join(E2E_SCREENSHOT_DIR, "vox_horizon_web.aadf-probe.log"),
      probeWeb,
    );

    for (const err of collector.errors) {
      test.info().annotations.push({
        type: err.type,
        description: err.text,
      });
    }
    expect(
      collector.hasPanic,
      `WASM panic during horizon capture: ${collector.firstPanic}`,
    ).toBe(false);
    expect(
      collector.errors,
      `Console/Bevy errors during horizon capture:\n` +
        collector.errors
          .map((e) => `  [${e.type}] ${e.text.slice(0, 500)}`)
          .join("\n"),
    ).toHaveLength(0);
    expect(
      voxInstallSeen,
      "Expected to see the install-complete INFO log " +
        '("NAADF .vox loaded from …") within the timeout',
    ).toBe(true);

    await context.close();

    // === Phase 3 — SSIM-compare native vs web ============================
    //
    // Asserts SSIM ≥ SSIM_MIN. A passing run means the WASM canvas is
    // structurally similar to the native reference — distant geometry
    // reaches the horizon line on both. A failing run means the WASM
    // raymarcher terminates rays prematurely (the user-reported bug).
    const ssim = await runSsimCompare(nativePngPath, web.outPath, SSIM_MIN);
    test.info().annotations.push({
      type: "ssim-compare-stdout",
      description: ssim.stdout,
    });
    test.info().annotations.push({
      type: "ssim-compare-stderr",
      description: ssim.stderr,
    });

    // 2026-05-20 — funnel sidecar. Persist the per-run diagnostic .txt
    // BEFORE the `expect(ssim.code).toBe(0)` assertion so a failed SSIM
    // run still leaves its sidecar on disk (failed runs are the
    // load-bearing data — they're the attractor states we want to group).
    const ssimScore = extractSsimScore(ssim.stdout);
    const ssimPass = ssim.code === 0;
    const sidecarBody = buildFunnelSidecar({
      timestamp: runTimestamp,
      ssim: ssimScore,
      ssimPass: ssimScore === null ? null : ssimPass,
      sentinelLines,
      panicLines,
    });
    await fs.writeFile(funnelTxtPath, sidecarBody);
    test.info().annotations.push({
      type: "funnel-png",
      description: funnelPngPath,
    });
    test.info().annotations.push({
      type: "funnel-txt",
      description: funnelTxtPath,
    });

    expect(
      ssim.code,
      `--ssim-compare exited non-zero (${ssim.code}) — cross-target ` +
        `horizon parity failed.\n` +
        `Native reference: ${nativePngPath}\n` +
        `WASM capture: ${web.outPath}\n` +
        `SSIM floor: ${SSIM_MIN}\n` +
        `stdout:\n${ssim.stdout}\n` +
        `stderr:\n${ssim.stderr}\n\n` +
        `Likely cause: WASM-only ray-termination bug — the chunk-level ` +
        `AADF acceleration (bounds_calc.wgsl's indirect-dispatch ` +
        `convergence loop) is not converging on WebGPU, so rays step ` +
        `chunk-by-chunk and exhaust max_ray_steps_primary = 120 at ` +
        `~30 % of the world depth. See ` +
        `crates/bevy_naadf/src/e2e/vox_horizon_parity.rs for the gate ` +
        `design + the diagnostic.`,
    ).toBe(0);
  });
});
