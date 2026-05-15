#!/bin/bash
# Rewrites the Trunk `dist/` so the wasm binary streams from R2 with a progress
# bar instead of loading inline. Run by the deploy CI after `trunk build`.
#
#   1. generate `dist/init.js` from `crates/bevy_naadf/init.js.template`
#      (fills in the Trunk JS bindings filename + the R2 wasm URL),
#   2. stamp the commit hash into `dist/sw.js`,
#   3. swap Trunk's inline module loader for `<script src="/init.js">` and
#      point the wasm preload at R2,
#   4. drop the now-unused local `*_bg.wasm` from `dist/`.
set -euo pipefail
DIST="${1:-crates/bevy_naadf/dist}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE_DIR="$(dirname "$SCRIPT_DIR")/crates/bevy_naadf"

COMMIT_HASH="${COMMIT_HASH:-$(git rev-parse --short=8 HEAD 2>/dev/null || echo 'dev')}"
R2="${R2_WASM_URL:-https://bevy-naadf-assets.yura415.workers.dev/bevy-naadf.wasm}?v=${COMMIT_HASH:0:8}"
WASM=$(basename "$DIST"/*_bg.wasm)
JS=$(grep -oP "from '/\K[^']+\.js" "$DIST/index.html")

# Generate init.js from the template.
sed -e "s|__JS_FILE__|$JS|g" \
    -e "s|__R2_URL__|$R2|g" \
    "$CRATE_DIR/init.js.template" > "$DIST/init.js"

# Inject the commit hash into the service worker cache name.
sed -i "s/__VERSION__/$COMMIT_HASH/g" "$DIST/sw.js"

# Swap Trunk's inline module loader for the streaming init.js, and repoint the
# wasm preload at R2.
sed -i "/<script type=\"module\">/,/<\/script>/c\\<script type=\"module\" src=\"/init.js\"></script>" "$DIST/index.html"
sed -i "s|href=\"/$WASM\"|href=\"$R2\"|g" "$DIST/index.html"

rm "$DIST"/*_bg.wasm
echo "Patched: $R2 (version: $COMMIT_HASH)"
