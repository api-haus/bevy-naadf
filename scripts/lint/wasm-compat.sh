#!/usr/bin/env bash
# Lint for std::time::Instant usage in WASM-compiled crates.
# Catches both `std::time::Instant` and `std::time::{..Instant..}` patterns.
# Instant::now() panics on WASM with "time not implemented on this platform".
# Use web_time::Instant instead — it works on both native and WASM.
set -euo pipefail

found=0
for dir in crates/bevy_naadf/src crates/voxel_noise/src; do
    # Match actual code usage, skip comment lines
    if grep -rn --include='*.rs' -E 'std::time::\{[^}]*Instant|std::time::Instant' "$dir" 2>/dev/null | grep -Ev '^[^:]+:[0-9]+:\s*//'; then
        echo "ERROR: std::time::Instant found in $dir (breaks WASM runtime)"
        found=1
    fi
done

if [ "$found" -eq 1 ]; then
    echo ""
    echo "Use web_time::Instant instead — it works on both native and WASM."
    exit 1
fi
echo "OK: No std::time::Instant in WASM-compiled crates."
