# bevy-naadf task runner. `just` with no arguments lists the recipes.

# Trunk dev-server bind address — keep in sync with Trunk.toml [serve].
web_host := "127.0.0.1"
web_port := "8080"
# The Chrome binary. WebGPU is the only wgpu browser backend that ships on
# Linux Chrome, and the NAADF render path requires it — so the web loop is
# Chrome-specific by design.
chrome := "google-chrome-stable"

# List available recipes.
default:
    @just --list

# Build the WebGPU (wasm32) build, serve it with Trunk, and open it in Chrome.
web:
    #!/usr/bin/env bash
    set -euo pipefail
    # Trunk watches the source tree and rebuilds on change; Ctrl-C stops the
    # server. Chrome is launched detached (setsid) so it stays open after.
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

# Build the WebGPU (wasm32) artifact into dist/ without serving.
web-build:
    trunk build

# Build the optimised (release) WebGPU artifact into dist/.
web-build-release:
    trunk build --release
