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
# It also sweeps the bundle ROOT. `dx build` copies `public/` verbatim and
# never deletes, so any file that ever lived in `public/` — a scratch page,
# a debug harness — stays in the dist forever and is embedded verbatim into
# the shipped app by Tauri. Two such files were found in the release
# bundle after being deleted from `public/` weeks earlier, which is the
# whole problem: removing the source is not enough, the artefact has to be
# swept too. A root file is kept only if `index.html` references it or it
# is in the allowlist below.
#
# Usage: scripts/prune-dx-dist.sh [dist-dir]
#   default: crates/wl-app/ui/target/dx/wl-ui/release/web/public
set -euo pipefail

DIST="${1:-crates/wl-app/ui/target/dx/wl-ui/release/web/public}"
# Resolve to an absolute path. A relative argument is taken from the repo
# root (this script lives in scripts/); an absolute one is honoured as
# given. The old resolution unconditionally prefixed the root, so passing
# an absolute dist dir — which the usage line invites — built a
# nonexistent path and the script exited 0 having pruned nothing, which
# reads as a clean pass.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
case "$DIST" in
    /*) ;;
    *) DIST="$ROOT/$DIST" ;;
esac
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
    fi
done < <(find "$DIST/assets" -maxdepth 1 -type f -name '*-dx*.wasm' -print0)

# ---------------------------------------------------------------------
# Root sweep
# ---------------------------------------------------------------------
# Files the bundle legitimately contains at the root. Anything else at
# depth 0 is a leftover: unreferenced, unreviewed, and shipped.
root_keep=(
    index.html      # the entry point
    wl.css          # design system
    invoke-shim.js  # IPC bridge (native AND mock)
)
while IFS= read -r -d '' stray; do
    name="$(basename "$stray")"
    keep=0
    for k in "${root_keep[@]}"; do
        [[ "$name" == "$k" ]] && { keep=1; break; }
    done
    if [[ "$keep" -eq 0 ]] && grep -qF "$name" "$INDEX"; then
        keep=1
    fi
    if [[ "$keep" -eq 1 ]]; then
        echo "prune-dx-dist: keeping $name"
    else
        echo "prune-dx-dist: removing unreferenced $name"
        rm -f "$stray"
    fi
done < <(find "$DIST" -maxdepth 1 -type f -print0)
