# bevy-naadf workspace task runner. `just` with no arguments lists the recipes.
#
# Workspace layout:
#   crates/bevy_naadf   — the Bevy 0.19 NAADF voxel renderer
#   crates/bevy-instamat — `material.ron` + PNG → StandardMaterial loader
#   crates/voxel_noise  — the FastNoise2 wrapper (native API + Emscripten module)

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
    setsid {{chrome}} --new-window "$url" >/dev/null 2>&1 &
    wait "$trunk_pid"

# Build the WebGPU (wasm32) artifact into crates/bevy_naadf/dist without serving.
web-build:
    cd {{naadf_dir}} && trunk build

# Build the optimised (release) WebGPU artifact into crates/bevy_naadf/dist.
web-build-release:
    cd {{naadf_dir}} && trunk build --release

# ── voxel_noise (FastNoise2 — native API + Emscripten C-ABI module) ─────────

# Build the voxel_noise Emscripten module → crates/voxel_noise/dist/ (needs emsdk).
noise-build:
    cd crates/voxel_noise && make build

# Run the voxel_noise native test suites.
noise-test:
    cargo test -p voxel_noise

# Clean the voxel_noise Emscripten build artifacts.
noise-clean:
    cd crates/voxel_noise && make clean

# ── e2e (Playwright smoke test against the web build) ───────────────────────

# Install the Playwright test dependencies + the chromium browser.
install-e2e:
    cd e2e && npm install && npx playwright install chromium

# Run the WASM e2e smoke test (requires a prior `just web-build-release`).
test-wasm:
    cd e2e && npx playwright test

# Run the WASM e2e smoke test with a visible browser (for debugging).
test-wasm-headed:
    cd e2e && npx playwright test --headed

# Build the release web artifact, then run the e2e smoke test against it.
test-wasm-full: web-build-release test-wasm
