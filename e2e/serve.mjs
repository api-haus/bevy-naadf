// Lightweight static file server for the bevy-naadf Trunk `dist/`.
//
// Sends the same COOP/COEP headers as the production `_headers` file so the
// page is cross-origin-isolated under test exactly as it is when deployed.

import { createServer } from "node:http";
import { createReadStream, statSync } from "node:fs";
import { join, extname, resolve, normalize } from "node:path";

const PORT = parseInt(process.env.PORT || "4173", 10);
const DIST_DIR = resolve(
  new URL(".", import.meta.url).pathname,
  "../crates/bevy_naadf/dist"
);
// Test fixtures (e.g. the Oasis .vox checked in at
// `crates/bevy_naadf/assets/test/`) served at `/test-fixtures/<name>` so
// the Playwright `.vox`-loading test can point `web_vox` at a same-origin
// file via the `?vox=<url>` query-string override — no dependency on the
// live R2 bucket having the right key.
const TEST_FIXTURES_DIR = resolve(
  new URL(".", import.meta.url).pathname,
  "../crates/bevy_naadf/assets/test"
);
const TEST_FIXTURES_PREFIX = "/test-fixtures/";

const MIME_TYPES = {
  ".html": "text/html",
  ".js": "application/javascript",
  ".mjs": "application/javascript",
  ".wasm": "application/wasm",
  ".css": "text/css",
  ".json": "application/json",
  ".wgsl": "text/plain",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".ktx2": "image/ktx2",
  ".svg": "image/svg+xml",
  ".ico": "image/x-icon",
  ".txt": "text/plain",
  ".toml": "text/plain",
  ".meta": "text/plain",
  ".vox": "application/octet-stream",
};

const server = createServer((req, res) => {
  // Mirror the deployed `_headers` — keeps the page cross-origin-isolated.
  res.setHeader("Cross-Origin-Opener-Policy", "same-origin");
  res.setHeader("Cross-Origin-Embedder-Policy", "require-corp");

  let urlPath = new URL(req.url, `http://localhost:${PORT}`).pathname;
  if (urlPath === "/") urlPath = "/index.html";

  // Resolve under either the dist tree or the test-fixtures tree, depending
  // on the URL prefix. Test fixtures live outside dist/ so we don't bloat
  // the deploy bundle with the 85 MB Oasis .vox.
  let baseDir;
  let relPath;
  if (urlPath.startsWith(TEST_FIXTURES_PREFIX)) {
    baseDir = TEST_FIXTURES_DIR;
    relPath = urlPath.slice(TEST_FIXTURES_PREFIX.length);
  } else {
    baseDir = DIST_DIR;
    relPath = urlPath;
  }

  // Directory traversal protection
  const filePath = normalize(join(baseDir, relPath));
  if (!filePath.startsWith(baseDir)) {
    res.writeHead(403);
    res.end("Forbidden");
    return;
  }

  let stat;
  try {
    stat = statSync(filePath);
  } catch {
    res.writeHead(404);
    res.end("Not Found");
    return;
  }

  if (!stat.isFile()) {
    res.writeHead(404);
    res.end("Not Found");
    return;
  }

  const ext = extname(filePath).toLowerCase();
  const contentType = MIME_TYPES[ext] || "application/octet-stream";

  res.writeHead(200, {
    "Content-Type": contentType,
    "Content-Length": stat.size,
  });

  createReadStream(filePath).pipe(res);
});

server.listen(PORT, () => {
  console.log(`Serving ${DIST_DIR} on http://localhost:${PORT}`);
});
