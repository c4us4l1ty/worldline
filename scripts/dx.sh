#!/usr/bin/env bash
# Resolves the Dioxus CLI (dx) and forwards arguments to it.
#
# Why this exists: /usr/bin/dx is commonly Open Visualization Data
# Explorer on Linux, which is NOT Dioxus. The real CLI installed by
# scripts/setup-linux.sh lives in ~/.cargo/bin/dx. Tauri hooks and
# humans must never resolve the wrong one silently.
set -euo pipefail

for candidate in "$HOME/.cargo/bin/dx" "$(command -v dx 2>/dev/null || true)"; do
    [ -n "$candidate" ] && [ -x "$candidate" ] || continue
    if "$candidate" --version 2>/dev/null | grep -qi dioxus; then
        exec "$candidate" "$@"
    fi
done

echo "error: no Dioxus CLI found (a non-Dioxus 'dx' may be shadowing it on PATH)." >&2
echo "  fix: export PATH=\"\$HOME/.cargo/bin:\$PATH\"  (or run scripts/setup-linux.sh)" >&2
exit 127
