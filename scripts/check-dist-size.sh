#!/usr/bin/env bash
# Enforces the release frontend size budget (SHIP-3).
#
# Why this exists separately from prune-dx-dist.sh: that script is a
# *staleness* pruner — it deletes content-hashed assets left behind by
# previous builds so the bundle does not grow without bound. It enforces
# no size at all, and exits 0 on anything it does not recognise. The
# plan had been treating it as a size guard, which it never was.
#
# This script is the actual budget. The number that matters for a
# local-first desktop app is the wasm payload the user downloads and the
# shell embeds: everything else (fonts, css, the shim) is small and
# predictable, while wasm is the thing that grows when a dependency
# gets heavier.
#
# Budget: 1 MiB of wasm. Current release build is ~824 KiB, so this has
# roughly 20% headroom — enough that routine dependency bumps do not
# break the build, tight enough that a runaway dependency does.
#
# Override with WL_MAX_WASM_BYTES for local experiments; CI should not.
# Usage: scripts/check-dist-size.sh [dist-dir]
set -euo pipefail

MAX_WASM_BYTES="${WL_MAX_WASM_BYTES:-1048576}"

DIST="${1:-crates/wl-app/ui/target/dx/wl-ui/release/web/public}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIST="$ROOT/${DIST#$ROOT/}"

if [[ ! -d "$DIST" ]]; then
    echo "check-dist-size: no bundle at $DIST — nothing to check" >&2
    exit 0
fi

total=0
while IFS= read -r -d '' wasm; do
    size=$(wc -c <"$wasm")
    name="$(basename "$wasm")"
    total=$((total + size))
    printf 'check-dist-size: %-44s %8d bytes\n' "$name" "$size"
done < <(find "$DIST" -type f -name '*.wasm' -print0)

if [[ "$total" -eq 0 ]]; then
    echo "check-dist-size: FAIL — no .wasm in $DIST" >&2
    exit 1
fi

if [[ "$total" -gt "$MAX_WASM_BYTES" ]]; then
    echo "check-dist-size: FAIL — wasm payload is $total bytes, over the" \
         "$MAX_WASM_BYTES-byte budget (SHIP-3)." >&2
    echo "  The shell embeds this verbatim, so it ships to every user." >&2
    echo "  Investigate with: du -sh \"$DIST\"" >&2
    exit 1
fi

# Report the total shipped footprint too — the wasm budget is the gate,
# but the whole dist is what actually lands on disk.
dist_total=$(du -sk "$DIST" | cut -f1)
printf 'check-dist-size: OK — wasm %d/%d bytes (%.0f%% of budget), dist %d KiB\n' \
    "$total" "$MAX_WASM_BYTES" \
    "$(awk -v a="$total" -v b="$MAX_WASM_BYTES" 'BEGIN { print a * 100 / b }')" \
    "$dist_total"
