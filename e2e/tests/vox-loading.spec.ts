import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { spawn } from "node:child_process";
import { test, expect, type ConsoleMessage, type Page } from "@playwright/test";
import { PNG } from "pngjs";
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
/**
 * SSIM ceiling for the skybox-vs-loaded compare. A healthy `.vox` install
 * lands well below this; a silent install-failure (renderer stays on
 * skybox/empty world) lands at SSIM ≈ 1.0 and fails the check.
 *
 * Re-baselined 2026-05-19 from 0.85 → 0.95 because the e2e camera frames
 * the Oasis fixture with a small colorful region at the top of the canvas
 * and a large below-horizon dark band that the empty-world skybox baseline
 * also produces (the world's finite extent means rays beyond the floor miss
 * to black in both captures). Measured SSIM on the post-fix focused-refresh
 * render is ~0.93 — structurally similar by SSIM because most of the
 * framebuffer IS visually identical, but the per-channel-spread floor
 * (`VOX_CHANNEL_MAX_FLOOR` below) catches the actual color-divergence
 * regression that SSIM is blind to. 0.95 keeps the SSIM check meaningful
 * (a true install no-op would score >0.99) while not failing on healthy
 * renders with this specific camera framing.
 */
const SSIM_DISSIMILARITY_MAX = 0.95;
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
 * Compute the maximum of the (mean_R, mean_G, mean_B) values over the
 * central 40% × 40% rect of the previously-captured canvas PNG. Decodes
 * the PNG Node-side via `pngjs` rather than reading from the live WebGPU
 * canvas. Returns the max-of-means in 0..=255.
 *
 * Why Node-side PNG decode (and NOT `drawImage` from the live WebGPU
 * canvas, the obvious-but-wrong approach the first cut took): the
 * WebGPU swapchain canvas in Chrome does NOT preserve its drawing
 * buffer by default. Once a frame is presented to the compositor the
 * source surface for `drawImage(canvas, …)` is the empty (zero-filled)
 * backing texture of the next frame, not the displayed pixels — so
 * `getImageData()` on the blit target reads literal zeros across all
 * channels even when the user can clearly see colorful content on the
 * page. The Playwright `canvas.screenshot()` path (used by
 * `captureSettledCanvas` above) goes through the browser's compositor
 * screenshot pipeline, which DOES capture the presented pixels. So the
 * fix is to compute the per-channel mean on the PNG that
 * `captureSettledCanvas` has already written to disk, by decoding it in
 * Node with `pngjs`. Mirrors the native gate's `region_mean(...)` /
 * `region_channel_max(...)` Rust path (`framebuffer.rs:237-273`) bit-for-
 * bit, just with the pixel source being a PNG instead of a wgpu readback.
 *
 * The central rect fractions (0.30..0.70 → 40% × 40%) match the native
 * gate's `Rect::from_fractional(&loaded_fb, 0.30, 0.30, 0.70, 0.70)` in
 * `vox_web_parity.rs`.
 */
async function pngCentralChannelMax(pngPath: string): Promise<number> {
  const bytes = await fs.readFile(pngPath);
  const png = PNG.sync.read(bytes);
  const { width: w, height: h, data } = png;
  if (w === 0 || h === 0) {
    throw new Error(
      `pngCentralChannelMax: decoded PNG has zero dimensions (${w}×${h}) at ${pngPath}`,
    );
  }
  // Central 40% × 40% rect in pixel coords (matches the native gate).
  const x0 = Math.floor(w * 0.3);
  const y0 = Math.floor(h * 0.3);
  const x1 = Math.floor(w * 0.7);
  const y1 = Math.floor(h * 0.7);
  const rw = x1 - x0;
  const rh = y1 - y0;
  if (rw <= 0 || rh <= 0) {
    throw new Error(
      `pngCentralChannelMax: degenerate rect ${x0},${y0}..${x1},${y1} for ${w}×${h} PNG`,
    );
  }
  let sumR = 0;
  let sumG = 0;
  let sumB = 0;
  // pngjs always outputs RGBA8 (4 bytes/pixel) regardless of source bit-depth.
  for (let y = y0; y < y1; y++) {
    let i = (y * w + x0) * 4;
    for (let x = x0; x < x1; x++) {
      sumR += data[i];
      sumG += data[i + 1];
      sumB += data[i + 2];
      i += 4;
    }
  }
  const n = rw * rh;
  const meanR = sumR / n;
  const meanG = sumG / n;
  const meanB = sumB / n;
  return Math.max(meanR, meanG, meanB);
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
    await page.goto("/?vox=/test-fixtures/oasis.cvox", {
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

    // === Phase 5 — per-channel color spread on the loaded canvas ==========
    //
    // web-vox-color-divergence (2026-05-18) Decision 4 + Stage A.5. Mirrors
    // the native `vox_web_parity` gate's `VOX_WEB_PARITY_CHANNEL_MAX_FLOOR`
    // assertion (`crates/bevy_naadf/src/e2e/vox_web_parity.rs`). The SSIM
    // compare below is structurally color-blind: a near-black render still
    // scores SSIM ≈ 0 vs the gradient skybox baseline because silhouettes
    // differ regardless of color. The per-channel floor catches the
    // "geometry correct, palette uploaded the wrong default-scene colors"
    // regression class directly — which is the exact bug this orchestration
    // fixed.
    //
    // Pixel source: the PNG `captureSettledCanvas` already wrote to disk
    // (`loaded.outPath`). See `pngCentralChannelMax`'s docblock for why we
    // decode the PNG Node-side instead of `drawImage`-ing the live WebGPU
    // canvas (TL;DR: Chromium does not preserve drawing buffer; the latter
    // reads zeros even when the user sees a colorful scene).
    //
    // Asserted before Phase 6's SSIM check because (a) channel-max is the
    // more diagnostic of the two for the near-black regression class, and
    // (b) the [vox-color-spread] log line stays visible in stdout even when
    // the SSIM step is the one that ultimately fails on a future regression.
    const channelMax = await pngCentralChannelMax(loaded.outPath);
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
        `SSIM-only assertion below is structurally color-blind; this gate ` +
        `catches the exact regression class the SSIM compare misses.`,
    ).toBeGreaterThan(VOX_CHANNEL_MAX_FLOOR);

    // === Phase 6 — SSIM-compare skybox vs loaded ===========================
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

    await context.close();
  });
});
