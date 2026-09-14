#!/usr/bin/env bash
# Worldline — Linux development environment setup.
# Installs Tauri v2 desktop build dependencies and CLI tools.
# Run once on a fresh machine:  bash scripts/setup-linux.sh
set -euo pipefail

echo "==> Installing Tauri v2 desktop build dependencies (requires sudo)…"
sudo apt-get update
sudo apt-get install -y \
    build-essential \
    curl \
    wget \
    file \
    libxdo-dev \
    libssl-dev \
    libayatana-appindicator3-dev \
    librsvg2-dev \
    libgtk-3-dev \
    libsoup-3.0-dev \
    libjavascriptcoregtk-4.1-dev \
    libwebkit2gtk-4.1-dev

echo "==> Installing Rust CLI tools (cargo install, may take several minutes)…"
if ! command -v cargo-tauri >/dev/null 2>&1; then
    cargo install tauri-cli --locked
else
    echo "    cargo-tauri already installed"
fi
if ! command -v dx >/dev/null 2>&1; then
    cargo install dioxus-cli --locked
else
    echo "    dioxus-cli (dx) already installed"
fi

echo "==> Verifying pkg-config finds the GTK/WebKit stacks…"
pkg-config --modversion webkit2gtk-4.1
pkg-config --modversion gtk+-3.0

echo
echo "Setup complete. Build the desktop client with:"
echo "  cd crates/wl-app && cargo tauri dev   # or: cargo tauri build"
