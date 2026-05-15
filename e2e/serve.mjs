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
};

const server = createServer((req, res) => {
  // Mirror the deployed `_headers` — keeps the page cross-origin-isolated.
  res.setHeader("Cross-Origin-Opener-Policy", "same-origin");
  res.setHeader("Cross-Origin-Embedder-Policy", "require-corp");

  let urlPath = new URL(req.url, `http://localhost:${PORT}`).pathname;
  if (urlPath === "/") urlPath = "/index.html";

  // Directory traversal protection
  const filePath = normalize(join(DIST_DIR, urlPath));
  if (!filePath.startsWith(DIST_DIR)) {
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
