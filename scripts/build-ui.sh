#!/usr/bin/env bash
# Release UI bundle for the Tauri shell: dx build + stale-asset prune.
# Used by `beforeBuildCommand` in crates/wl-app/tauri.conf.json.
# Cwd-independent (resolves the repo root from this script's location).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT/crates/wl-app/ui"
"$ROOT/scripts/dx.sh" build --release --debug-symbols false
"$ROOT/scripts/prune-dx-dist.sh"
