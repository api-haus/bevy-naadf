# bevy-naadf workspace task runner. `just` with no arguments lists the recipes.
#
# Workspace layout:
#   crates/bevy_naadf   — the Bevy 0.19 NAADF voxel renderer

# Trunk dev-server bind address — keep in sync with crates/bevy_naadf/Trunk.toml.
web_host := "127.0.0.1"
web_port := "8080"
# The Chrome binary. WebGPU is the only wgpu browser backend that ships on
# Linux Chrome, and the NAADF render path requires it — so the web loop is
# Chrome-specific by design.
chrome := "google-chrome-stable"
# The bevy_naadf crate dir — Trunk.toml + index.html live here.
naadf_dir := "crates/bevy_naadf"

# List available recipes.
default:
    @just --list

# ── Native ──────────────────────────────────────────────────────────────────

# Build the whole workspace (native).
build:
    cargo build --workspace

# Run the production renderer (needs the DLSS SDK env — see README / .envrc).
run:
    cargo run -p bevy-naadf --release

dev:
    cargo run -p bevy-naadf

# Bake `*.texarray.ron` definitions → Basis `.basis` arrays under
# `imported_assets/` (headless AssetProcessor; no GPU/DLSS needed). See README.
bake-texarrays:
    cargo run -p bevy-naadf --bin bake --no-default-features --release

# Run the workspace test suites.
test:
    cargo test --workspace

# Format all Rust code.
fmt:
    cargo fmt --all

# Check formatting without modifying files.
fmt-check:
    cargo fmt --all -- --check

# Run clippy across the workspace.
lint:
    cargo clippy --workspace --all-targets -- -D warnings

# Lint for std::time::Instant in WASM-compiled crates (use web_time instead).
lint-wasm-compat:
    ./scripts/lint/wasm-compat.sh

# ── bevy_naadf web build (wasm32-unknown-unknown / WebGPU, via Trunk) ────────

# Build the WebGPU (wasm32) build, serve it with Trunk, and open it in Chrome.
web:
    #!/usr/bin/env bash
    set -euo pipefail
    # Trunk watches the source tree and rebuilds on change; Ctrl-C stops the
    # server. Chrome is launched detached (setsid) so it stays open after.
    cd {{naadf_dir}}
    url="http://{{web_host}}:{{web_port}}"
    echo "trunk serve → $url   (wasm32 / WebGPU, Chrome)"
    trunk serve &
    trunk_pid=$!
    trap 'kill "$trunk_pid" 2>/dev/null || true' EXIT
    # Wait for the dev server to answer before launching the browser.
    for _ in $(seq 1 150); do
        if curl -sf -o /dev/null "$url"; then break; fi
        sleep 0.2
    done
    # `setsid` detaches Chrome into its own session so Ctrl-C on the dev
    # server does not also signal the browser.
    # `--enable-unsafe-webgpu --enable-webgpu-developer-features` mirror the
    # Playwright config (`e2e/playwright.config.ts:44-48`) so live dev and
    # the SSIM gate exercise the same WebGPU surface; the dev-features flag
    # also surfaces Dawn validation errors as page errors.
    setsid {{chrome}} \
        --new-window \
        --enable-unsafe-webgpu \
        --enable-webgpu-developer-features \
        "$url" >/dev/null 2>&1 &
    wait "$trunk_pid"

# Build the WebGPU (wasm32) artifact into crates/bevy_naadf/dist without serving.
web-build:
    cd {{naadf_dir}} && trunk build

# Build the optimised (release) WebGPU artifact into crates/bevy_naadf/dist.
web-build-release:
    cd {{naadf_dir}} && trunk build --release

# Serve a pre-built `dist/` with miniserve + the COOP/COEP headers wasm-bindgen-rayon
# needs for SharedArrayBuffer, then open Chrome. No file watching, no live reload —
# escape hatch for when `trunk serve` blows up on the inotify limit. Run
# `just web-build` (or `web-build-release`) first; rerun it when you want a refresh.
# Requires miniserve: `cargo install miniserve`.
web-static: web-build-release
    #!/usr/bin/env bash
    set -euo pipefail
    cd {{naadf_dir}}
    if [[ ! -f dist/index.html ]]; then
        echo "dist/ is empty — run \`just web-build\` or \`just web-build-release\` first." >&2
        exit 1
    fi
    url="http://{{web_host}}:{{web_port}}"
    lan_ip="$(ip route get 1.1.1.1 2>/dev/null | awk '{for(i=1;i<=NF;i++) if($i=="src"){print $(i+1); exit}}')"
    echo "miniserve dist → $url   (static, no watch, COOP/COEP enabled)"
    if [[ -n "$lan_ip" ]]; then
        echo "                LAN: http://$lan_ip:{{web_port}}   (phone / tablet on same network)"
    fi
    miniserve dist \
        --index index.html \
        --interfaces 0.0.0.0 \
        --port {{web_port}} \
        --header "Cross-Origin-Opener-Policy: same-origin" \
        --header "Cross-Origin-Embedder-Policy: require-corp" &
    serve_pid=$!
    trap 'kill "$serve_pid" 2>/dev/null || true' EXIT
    for _ in $(seq 1 150); do
        if curl -sf -o /dev/null "$url"; then break; fi
        sleep 0.2
    done
    # `--enable-unsafe-webgpu --enable-webgpu-developer-features` mirror the
    # Playwright config (`e2e/playwright.config.ts:44-48`) so live dev and
    # the SSIM gate exercise the same WebGPU surface.
    setsid {{chrome}} \
        --new-window \
        --enable-unsafe-webgpu \
        --enable-webgpu-developer-features \
        "$url" >/dev/null 2>&1 &
    wait "$serve_pid"

# ── e2e (Playwright smoke test against the web build) ───────────────────────
#
# ALWAYS run the Playwright e2e suite headful.
#
# The NAADF render path is WebGPU-only and uses a heavy compute pipeline that
# overruns headless Chromium's `chrome-headless-shell` WebGPU stack (SwiftShader
# fallback only) — the device times out and panics mid-render with
# `Caught DeviceLost error: Destroyed Device was destroyed.` before the .vox
# install can even complete. That noise hides the *real* failures we want the
# suite to surface (wgpu validation, buffer-flag mismatches, wasm panics, ...).
#
# Headed Chrome routes through the same Dawn + GPU-process pipeline as a
# normal browser session, picks the host adapter, and reaches the same
# render state a user does — so the suite catches the bugs a user would.
#
# `test-wasm` and `test-wasm-full` therefore both run in headed mode by
# default. A separate `test-wasm-headless` recipe stays for the rare case
# where someone wants to triage the headless-only failure modes.

# Install the Playwright test dependencies + the chromium browser.
install-e2e:
    cd e2e && npm install && npx playwright install chromium

# Run the WASM e2e suite (requires a prior `just web-build-release`).
# Headed by default — see the block comment above for why.
test-wasm:
    cd e2e && npx playwright test --headed

# Diagnostic-only — expected to fail with WebGPU `DeviceLost` (see block comment).
test-wasm-headless:
    cd e2e && npx playwright test

# Build the release web artifact, then run the e2e suite (headed) against it.
test-wasm-full: web-build-release test-wasm

