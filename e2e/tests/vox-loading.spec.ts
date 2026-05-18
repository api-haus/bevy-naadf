import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import { test, expect, type ConsoleMessage, type Page } from "@playwright/test";
import { ConsoleCollector } from "./helpers/console-collector.js";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

/**
 * Shared tempdir used by the skybox-baseline test to publish its PNG path
 * to the loaded-phase test. Lives under the OS tmpdir so it survives
 * across the two test cases inside the same `test.describe.serial` block.
 */
const SHARED_TMP_DIR = path.join(
  os.tmpdir(),
  `bevy-naadf-vox-parity-${process.pid}`,
);

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
 *  ## web-vox-async-loading 2026-05-18 follow-up Step 9 / Q6
 *
 *  Additionally captures a **skybox-only baseline** via the `?skybox=1` URL
 *  param + a **post-install loaded screenshot** and shells out to
 *  `cargo run --bin e2e_render -- --ssim-compare <skybox> <loaded>
 *  --ssim-max 0.85` to assert the two are **dissimilar** (SSIM < 0.85). The
 *  Rust binary's SSIM impl is shared with the native `--vox-web-parity`
 *  gate (per Decision 4: zero metric drift between web + native).
 */

const REPO_ROOT = path.resolve(__dirname, "..", "..");
const SSIM_DISSIMILARITY_MAX = 0.85;
const CANVAS_SETTLE_MS = 10_000;
const SKYBOX_SETTLE_MS = 5_000;

/**
 * web-vox-color-divergence (2026-05-18) Decision 4 + Stage A.5 — per-channel
 * mean-max floor for the loaded canvas's central 40% × 40% rect. Mirrors the
 * native gate's `VOX_WEB_PARITY_CHANNEL_MAX_FLOOR` (30.0 on 0..255 scale) so
 * the wasm test fails when the .vox install path produces structurally-correct
 * but colorless / near-black voxels — the exact regression class the
 * SSIM-only check is structurally blind to.
 */
const VOX_CHANNEL_MAX_FLOOR = 30.0;

/**
 * Compute the mean-of-channel-maximum over the central 40% × 40% rect of the
 * canvas's currently-visible framebuffer, by drawing the WebGPU canvas onto
 * an in-page 2D canvas and reading its `ImageData`. Returns the maximum of
 * the (mean_R, mean_G, mean_B) values, each in 0..255.
 *
 * Why this approach: the WebGPU canvas's `getContext("webgpu")` is the
 * production context; calling `getContext("2d")` on the same element returns
 * null (browsers forbid context-type switching). `drawImage(canvas, …)` from
 * the WebGPU canvas onto a separate 2D canvas is the supported readback
 * path (Chrome respects the canvas's `preserveDrawingBuffer` heuristic for
 * the most recent presented frame).
 */
async function canvasCentralChannelMax(page: Page): Promise<number> {
  return await page.evaluate(() => {
    const canvas = document.querySelector<HTMLCanvasElement>("canvas#bevy");
    if (!canvas) {
      throw new Error("canvasCentralChannelMax: canvas#bevy not found");
    }
    const w = canvas.width;
    const h = canvas.height;
    if (w === 0 || h === 0) {
      throw new Error(
        `canvasCentralChannelMax: canvas has zero dimensions (${w}×${h})`,
      );
    }
    // Central 40% × 40% rect in pixel coords.
    const x0 = Math.floor(w * 0.3);
    const y0 = Math.floor(h * 0.3);
    const x1 = Math.floor(w * 0.7);
    const y1 = Math.floor(h * 0.7);
    const rw = x1 - x0;
    const rh = y1 - y0;
    if (rw <= 0 || rh <= 0) {
      throw new Error(
        `canvasCentralChannelMax: degenerate rect ${x0},${y0}..${x1},${y1}`,
      );
    }
    // Blit JUST the central rect onto a small 2D canvas so we don't allocate
    // a full-canvas-sized ImageData buffer.
    const blit = document.createElement("canvas");
    blit.width = rw;
    blit.height = rh;
    const ctx = blit.getContext("2d");
    if (!ctx) {
      throw new Error("canvasCentralChannelMax: 2D context unavailable");
    }
    ctx.drawImage(canvas, x0, y0, rw, rh, 0, 0, rw, rh);
    const img = ctx.getImageData(0, 0, rw, rh);
    const data = img.data;
    let sumR = 0;
    let sumG = 0;
    let sumB = 0;
    const n = rw * rh;
    for (let i = 0; i < data.length; i += 4) {
      sumR += data[i];
      sumG += data[i + 1];
      sumB += data[i + 2];
    }
    const meanR = sumR / n;
    const meanG = sumG / n;
    const meanB = sumB / n;
    return Math.max(meanR, meanG, meanB);
  });
}

/**
 * Capture a screenshot of the wasm canvas after the loading overlay clears
 * and the scene has settled for `settleMs` milliseconds. Returns the PNG
 * bytes plus the test-run-local path it was written to.
 */
async function captureSettledCanvas(
  page: Page,
  filename: string,
  settleMs: number,
  outDir?: string,
): Promise<{ bytes: Buffer; outPath: string }> {
  // Wait for `#loading.hidden` to be attached so we don't capture the
  // overlay covering the canvas. Generous timeout — the wasm boot streams
  // ~50 MB on first load.
  const loadingHidden = page.locator("#loading.hidden");
  await expect(loadingHidden).toBeAttached({ timeout: 90_000 });
  // Make sure the canvas is in the DOM and laid out.
  const canvas = page.locator("canvas#bevy");
  await expect(canvas).toBeVisible({ timeout: 10_000 });
  // Let the renderer draw enough frames to settle (TAA convergence, GI
  // accumulation, the W5 GPU producer chain dispatch + the Q3 cross-frame
  // CPU mirror readback all complete inside this window).
  await page.waitForTimeout(settleMs);
  // Screenshot, attach, write to disk for the SSIM compare step.
  const bytes = await canvas.screenshot();
  await test.info().attach(filename, { body: bytes, contentType: "image/png" });
  const targetDir = outDir ?? test.info().outputDir;
  await fs.mkdir(targetDir, { recursive: true });
  const outPath = path.join(targetDir, filename);
  await fs.writeFile(outPath, bytes);
  return { bytes, outPath };
}

/**
 * Shell out to `cargo run --bin e2e_render -- --ssim-compare <a> <b>
 * --ssim-max <max>`. Returns the exit code and captured stdout/stderr.
 *
 * Exit-code semantics (per `crates/bevy_naadf/src/e2e/ssim.rs`):
 *   - `0` — gate passed.
 *   - `1` — gate failed (SSIM >= --ssim-max).
 *   - `2` — internal error (file not found, decode error, dimension
 *     mismatch).
 */
async function runSsimCompare(
  a: string,
  b: string,
  max: number,
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
        "--ssim-max",
        String(max),
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

test.describe.serial("Web .vox loading", () => {
  // Skybox-baseline PNG is captured in a dedicated test (separate browser
  // context so the wasm workers from the loaded-phase don't conflict). The
  // path is published into a process-global variable that the loaded-phase
  // test reads + the SSIM-compare step uses.
  let skyboxBaselinePath: string | undefined;

  // Use a fresh browser context per test to avoid wasm-worker / SAB state
  // leakage between the skybox and loaded phases (each navigation involves
  // a wasm-bindgen-rayon worker pool spawn; re-using the same context
  // races the worker teardown against the next navigation's worker
  // setup and Chrome reports "Failed to fetch dynamically imported module").
  test.use({ contextOptions: {} });

  test("captures skybox baseline via ?skybox=1", async ({ browser }) => {
    const context = await browser.newContext();
    const page = await context.newPage();
    const collector = new ConsoleCollector();
    collector.attach(page);

    // web-vox-color-divergence diagnose-first (2026-05-18) — forward the
    // one-shot palette-trace lines to Node stdout for the diagnose phase.
    page.on("console", (msg: ConsoleMessage) => {
      const text = msg.text();
      if (
        text.includes("[palette-upload]") ||
        text.includes("[palette-install]")
      ) {
        // eslint-disable-next-line no-console
        console.log(`[wasm-console] ${text}`);
      }
    });

    // Boot the page with `?skybox=1`. The wasm bootstrap reads the param
    // via `voxel::web_vox::resolve_skybox_only_param`, inserts a
    // `WebSkyboxOverride` resource, and `setup_test_grid` installs an
    // empty world (renderer produces a pure-sky frame).
    await page.goto("/?skybox=1", { waitUntil: "commit" });
    const skybox = await captureSettledCanvas(
      page,
      "canvas-skybox-baseline.png",
      SKYBOX_SETTLE_MS,
      SHARED_TMP_DIR,
    );
    skyboxBaselinePath = skybox.outPath;

    // The skybox phase shouldn't produce console errors / panics; any
    // that do are real and should fail this test (independently of the
    // loaded-phase below).
    for (const err of collector.errors) {
      test.info().annotations.push({
        type: err.type,
        description: err.text,
      });
    }
    expect(
      collector.hasPanic,
      `WASM panic during skybox baseline capture: ${collector.firstPanic}`,
    ).toBe(false);
    await context.close();
  });

  test("startup-fetches and installs the default .vox without errors, then SSIM-asserts dissimilar from skybox baseline", async ({
    browser,
  }) => {
    expect(
      skyboxBaselinePath,
      "skybox baseline test must have run first (use test.describe.serial)",
    ).toBeDefined();

    const context = await browser.newContext();
    const page = await context.newPage();
    const collector = new ConsoleCollector();
    collector.attach(page);

    // Watch for the install-complete log line emitted by
    // `install_imported_vox` ("NAADF .vox loaded from …"). The tracing-wasm
    // bridge emits it as `console.log` with `%cINFO%c` CSS markers; the
    // literal message text is present.
    let voxInstallSeen = false;
    page.on("console", (msg: ConsoleMessage) => {
      const text = msg.text();
      if (text.includes("NAADF .vox loaded from")) {
        voxInstallSeen = true;
      }
      // web-vox-color-divergence diagnose-first (2026-05-18) — forward the
      // one-shot [palette-upload] / [palette-install] diagnostic lines from
      // the wasm tracing bridge to Node stdout so `just test-wasm 2>&1 | tee`
      // captures them. Remove (or scope) once the diagnose phase concludes.
      if (text.includes("[palette-upload]") || text.includes("[palette-install]")) {
        // eslint-disable-next-line no-console
        console.log(`[wasm-console] ${text}`);
      }
    });

    // `?vox=<url>` override (parsed by
    // `voxel::web_vox::resolve_startup_vox_url`) — point at a same-origin
    // copy of the Oasis fixture served by `serve.mjs` under
    // `/test-fixtures/`. The default URL targets the live R2 bucket which
    // may not have the right key uploaded.
    await page.goto("/?vox=/test-fixtures/oasis_hard_cover.vox", {
      waitUntil: "commit",
    });

    // Phase 2a — wasm init complete.
    const loadingHidden = page.locator("#loading.hidden");
    await expect(loadingHidden).toBeAttached({ timeout: 90_000 });

    // Phase 2b — canvas visible.
    const canvas = page.locator("canvas#bevy");
    await expect(canvas).toBeVisible({ timeout: 10_000 });

    // Phase 2c — wait for one of three terminal conditions:
    //   (a) the install-complete INFO log fires (happy path),
    //   (b) a wasm panic fires,
    //   (c) any console.error / Bevy ERROR / pageerror lands.
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

    // Phase 2d — let TAA/GI/Q3 readback settle, then capture.
    const loaded = await captureSettledCanvas(
      page,
      "canvas-after-vox-install.png",
      CANVAS_SETTLE_MS,
      SHARED_TMP_DIR,
    );

    // === Phase 3 — Report errors as annotations ============================
    for (const err of collector.errors) {
      test.info().annotations.push({
        type: err.type,
        description: err.text,
      });
    }

    // === Phase 4 — Error / panic / install-seen assertions =================
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

    // === Phase 5 — SSIM-compare skybox vs loaded ===========================
    //
    // Shell out to `cargo run --bin e2e_render -- --ssim-compare` for
    // metric-parity with the native `--vox-web-parity` gate (Decision 4).
    // Asserts SSIM < `SSIM_DISSIMILARITY_MAX` — a silent failure mode
    // (e.g. install path no-ops, renders sky) would land at SSIM ≈ 1.0
    // and fail; a healthy install lands far below the threshold.
    const ssim = await runSsimCompare(
      skyboxBaselinePath!,
      loaded.outPath,
      SSIM_DISSIMILARITY_MAX,
    );
    test.info().annotations.push({
      type: "ssim-compare-stdout",
      description: ssim.stdout,
    });
    test.info().annotations.push({
      type: "ssim-compare-stderr",
      description: ssim.stderr,
    });

    expect(
      ssim.code,
      `--ssim-compare exited non-zero (${ssim.code}) — stdout:\n${ssim.stdout}\nstderr:\n${ssim.stderr}`,
    ).toBe(0);

    // === Phase 6 — per-channel color spread on the loaded canvas ===========
    //
    // web-vox-color-divergence (2026-05-18) Decision 4 + Stage A.5. Mirrors
    // the native `vox_web_parity` gate's `VOX_WEB_PARITY_CHANNEL_MAX_FLOOR`
    // assertion (`crates/bevy_naadf/src/e2e/vox_web_parity.rs`). The SSIM
    // compare above is structurally color-blind: a near-black render still
    // scores SSIM ≈ 0 vs the gradient skybox baseline because silhouettes
    // differ regardless of color. The per-channel floor catches the
    // "geometry correct, palette uploaded the wrong default-scene colors"
    // regression class directly — which is the exact bug this orchestration
    // fixed.
    const channelMax = await canvasCentralChannelMax(page);
    const channelMaxLine = `[vox-color-spread] loaded canvas central rect channel max = ${channelMax.toFixed(1)} (threshold > ${VOX_CHANNEL_MAX_FLOOR.toFixed(0)})`;
    // eslint-disable-next-line no-console
    console.log(channelMaxLine);
    test.info().annotations.push({
      type: "vox-color-spread",
      description: channelMaxLine,
    });
    expect(
      channelMax,
      `near-black voxel render — web-vox-color-divergence regression class. ` +
        `Loaded canvas central rect channel max = ${channelMax.toFixed(1)} ` +
        `(threshold > ${VOX_CHANNEL_MAX_FLOOR.toFixed(0)}). The .vox install ` +
        `path produced structurally correct geometry but colorless / near-black ` +
        `voxels — likely a regression in the focused-refresh path in ` +
        `crates/bevy_naadf/src/render/prepare.rs's prepare_world_gpu. The ` +
        `SSIM-only assertion above is structurally color-blind; this gate ` +
        `catches the exact regression class the SSIM compare misses.`,
    ).toBeGreaterThan(VOX_CHANNEL_MAX_FLOOR);

    await context.close();
  });
});
