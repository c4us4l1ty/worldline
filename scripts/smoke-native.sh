#!/usr/bin/env bash
# Native shell smoke test: launches the Tauri binary with an isolated data
# dir, focuses the window via KWin scripting, captures it, and asserts the
# webview is NOT blank (pixel-variance + canvas-color fraction check).
#
# Usage: scripts/smoke-native.sh [path-to-wl-app-binary]
# Exit 0 = window renders; 1 = blank window / launch failure.
# Requires: KDE (KWin + spectacle), python3. No display server changes.
set -euo pipefail

BIN="${1:-crates/wl-app/target/release/wl-app}"
[[ -x "$BIN" ]] || { echo "FAIL: binary not found: $BIN"; exit 1; }

DATA="$(mktemp -d /tmp/worldline-smoke-XXXXXX)"
SHOT="$(mktemp /tmp/worldline-shot-XXXXXX.png)"
LOG="$(mktemp /tmp/worldline-log-XXXXXX.txt)"
SCRIPT_ID=""
PID=""
cleanup() {
    [[ -n "$PID" ]] && kill "$PID" 2>/dev/null || true
    # Unload the KWin script we loaded.
    if [[ -n "$SCRIPT_ID" ]]; then
        gdbus call --session --dest org.kde.KWin \
            --object-path "/Scripting/Script$SCRIPT_ID" \
            --method org.kde.kwin.Script.stop >/dev/null 2>&1 || true
    fi
    rm -rf "$DATA" "$SHOT" "$LOG"
}
trap cleanup EXIT

XDG_DATA_HOME="$DATA" "$BIN" >"$LOG" 2>&1 &
PID=$!

# KWin script: focus the Worldline window.
cat > "$DATA/activate.js" <<'JS'
for (const w of workspace.windowList()) {
    if (w.caption && w.caption.indexOf("Worldline") !== -1) {
        if (w.minimized) w.minimized = false;
        workspace.activeWindow = w;
        print("wl-smoke-activated");
    }
}
JS

# Wait for the window to appear (max 20s), then focus it.
ACTIVATED=0
for _ in $(seq 1 40); do
    kill -0 "$PID" 2>/dev/null || { echo "FAIL: process died"; cat "$LOG"; exit 1; }
    if [[ "$ACTIVATED" -eq 0 ]]; then
        OUT=$(gdbus call --session --dest org.kde.KWin \
            --object-path /Scripting \
            --method org.kde.kwin.Scripting.loadScript \
            "$DATA/activate.js" "wlsmoke$RANDOM" 2>/dev/null || true)
        SCRIPT_ID=$(echo "$OUT" | grep -oE '^\(uint32 [0-9]+' | grep -oE '[0-9]+' || true)
        if [[ -n "$SCRIPT_ID" ]]; then
            gdbus call --session --dest org.kde.KWin \
                --object-path "/Scripting/Script$SCRIPT_ID" \
                --method org.kde.kwin.Script.run >/dev/null 2>&1 || true
            ACTIVATED=1
        fi
    fi
    sleep 0.5
done
[[ "$ACTIVATED" -eq 1 ]] || { echo "FAIL: could not load KWin activation script"; cat "$LOG"; exit 1; }
sleep 1.5

spectacle --background --activewindow -n -o "$SHOT" 2>/dev/null
[[ -s "$SHOT" ]] || { echo "FAIL: capture produced no image"; exit 1; }

python3 - "$SHOT" <<'PY'
import sys, zlib, struct


def read_png(path):
    data = open(path, "rb").read()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "not a png"
    pos, w, h, ctype = 8, 0, 0, 0
    idat = b""
    while pos < len(data):
        (ln,) = struct.unpack(">I", data[pos:pos + 4])
        typ = data[pos + 4:pos + 8]
        chunk = data[pos + 8:pos + 8 + ln]
        if typ == b"IHDR":
            w, h, _, ctype = struct.unpack(">IIBB", chunk[:10])
        elif typ == b"IDAT":
            idat += chunk
        pos += 12 + ln
    raw = zlib.decompress(idat)
    ch = {0: 1, 2: 3, 4: 2, 6: 4}[ctype]
    stride = w * ch
    out = bytearray()
    prev = bytearray(stride)
    i = 0
    for _ in range(h):
        f = raw[i]
        i += 1
        line = bytearray(raw[i:i + stride])
        i += stride
        for x in range(stride):
            a = line[x - ch] if x >= ch else 0
            b = prev[x]
            c = prev[x - ch] if x >= ch else 0
            if f == 1:
                line[x] = (line[x] + a) & 0xFF
            elif f == 2:
                line[x] = (line[x] + b) & 0xFF
            elif f == 3:
                line[x] = (line[x] + (a + b) // 2) & 0xFF
            elif f == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[x] = (line[x] + pr) & 0xFF
        out += line
        prev = line
    return w, h, ch, bytes(out)


w, h, ch, px = read_png(sys.argv[1])
n = w * h
sums = [0] * ch
sqs = [0] * ch
canvas_hits = 0
for i in range(0, len(px), ch):
    for c in range(ch):
        v = px[i + c]
        sums[c] += v
        sqs[c] += v * v
    if ch >= 3:
        # Near-canvas #131312 (19,19,18) within +-6/channel.
        r, g, b = px[i], px[i + 1], px[i + 2]
        if all(abs(v - t) <= 6 for v, t in ((r, 19), (g, 19), (b, 18))):
            canvas_hits += 1
max_sd = 0.0
for c in range(min(3, ch)):
    mean = sums[c] / n
    var = sqs[c] / n - mean * mean
    max_sd = max(max_sd, var ** 0.5)
frac = canvas_hits / n if ch >= 3 else 0.0
print(f"window {w}x{h}, channel stddev {max_sd:.1f}, canvas fraction {frac:.2f}")
if not (380 <= w <= 1400 and 500 <= h <= 1800):
    print("FAIL: captured window has unexpected size (wrong window?)")
    sys.exit(1)
if max_sd < 8.0:
    print("FAIL: window is blank (uniform pixels)")
    sys.exit(1)
if frac < 0.25:
    print("FAIL: canvas color #131312 barely present (wrong window?)")
    sys.exit(1)
print("PASS: Worldline window renders the design-system canvas")
PY
