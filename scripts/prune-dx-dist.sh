#!/usr/bin/env bash
# Prunes stale hashed dx assets from a built frontend dist.
#
# `dx build` content-hashes wasm/js bundles (wl-ui-dx<hash>.js) but never
# deletes superseded ones, so every rebuild permanently adds ~800 KiB of
# dead weight that Tauri embeds into the shipped app. This keeps exactly
# the assets reachable from index.html (js directly, wasm via the kept
# js) and deletes only `*-dx*` files under assets/ — index.html, wl.css,
# invoke-shim.js, and fonts/ are never touched.
#
# Usage: scripts/prune-dx-dist.sh [dist-dir]
#   default: crates/wl-app/ui/target/dx/wl-ui/release/web/public
set -euo pipefail

DIST="${1:-crates/wl-app/ui/target/dx/wl-ui/release/web/public}"
# Resolve relative to the repo root (this script lives in scripts/).
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST="$ROOT/${DIST#$ROOT/}"
INDEX="$DIST/index.html"

[[ -f "$INDEX" ]] || { echo "prune-dx-dist: no bundle at $DIST (nothing to prune)"; exit 0; }
[[ -d "$DIST/assets" ]] || exit 0

kept_js=()
while IFS= read -r -d '' js; do
    name="$(basename "$js")"
    if grep -qF "$name" "$INDEX"; then
        kept_js+=("$js")
    else
        echo "prune-dx-dist: removing stale $name"
        rm -f "$js"
    fi
done < <(find "$DIST/assets" -maxdepth 1 -type f -name '*-dx*.js' -print0)

while IFS= read -r -d '' wasm; do
    name="$(basename "$wasm")"
    keep=0
    for js in ${kept_js[@]+"${kept_js[@]}"}; do
        if grep -qF "$name" "$js"; then keep=1; break; fi
    done
    if [[ "$keep" -eq 1 ]]; then
        echo "prune-dx-dist: keeping $name"
    else
        echo "prune-dx-dist: removing stale $name"
        rm -f "$wasm"
    fi
done < <(find "$DIST/assets" -maxdepth 1 -type f -name '*-dx*.wasm' -print0)
