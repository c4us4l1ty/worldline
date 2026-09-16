#!/usr/bin/env bash
# Worldline — Fedora setup (faster binary install, skips cargo install compile)
# Uses dnf + pre-built tauri-cli / dioxus-cli binaries.
set -euo pipefail

echo "==> Installing Tauri v2 desktop build dependencies (dnf)…"
sudo dnf install -y \
    webkit2gtk4.1-devel \
    gtk3-devel \
    libsoup3-devel \
    librsvg2-devel \
    libxdo-devel \
    openssl-devel \
    file \
    curl \
    wget \
    pkg-config

echo "==> Checking / installing CLI binaries (pre-built, no cargo compile)…"

TAURI_BIN_DIR="${HOME}/.local/bin"
mkdir -p "${TAURI_BIN_DIR}"

# Tauri CLI v2 binary (repo uses tauri v2)
TAURI_VERSION="v2.5.0"
TAURI_URL="https://github.com/tauri-apps/tauri/releases/download/tauri-cli-${TAURI_VERSION}/cargo-tauri-x86_64-unknown-linux-gnu.tgz"
TAURI_TARGET="${TAURI_BIN_DIR}/cargo-tauri"

if command -v cargo-tauri >/dev/null 2>&1 || [ -x "${TAURI_TARGET}" ]; then
    echo "    tauri-cli already installed"
else
    echo "    Downloading tauri-cli ${TAURI_VERSION} binary…"
    curl -L -o /tmp/tauri-cli.tgz "${TAURI_URL}"
    tar -xzf /tmp/tauri-cli.tgz -C /tmp cargo-tauri
    chmod +x /tmp/cargo-tauri
    mv /tmp/cargo-tauri "${TAURI_TARGET}"
    rm -f /tmp/tauri-cli.tgz
    echo "    Installed to ${TAURI_TARGET}"
fi

# Dioxus CLI binary (repo uses dioxus v0.7)
DIOXUS_VERSION="v0.7.10"
DIOXUS_URL="https://github.com/DioxusLabs/dioxus/releases/download/${DIOXUS_VERSION}/dx-x86_64-unknown-linux-gnu.tar.gz"
DIOXUS_TARGET="${TAURI_BIN_DIR}/dx"

if command -v dx >/dev/null 2>&1 || [ -x "${DIOXUS_TARGET}" ]; then
    echo "    dioxus-cli (dx) already installed"
else
    echo "    Downloading dioxus-cli ${DIOXUS_VERSION} binary…"
    curl -L -o /tmp/dx.tar.gz "${DIOXUS_URL}"
    tar -xzf /tmp/dx.tar.gz -C /tmp dx
    chmod +x /tmp/dx
    mv /tmp/dx "${DIOXUS_TARGET}"
    rm -f /tmp/dx.tar.gz
    echo "    Installed to ${DIOXUS_TARGET}"
fi

# Ensure local bin is on PATH for this session and future shells
if [[ ":${PATH}:" != *":${TAURI_BIN_DIR}:"* ]]; then
    export PATH="${TAURI_BIN_DIR}:${PATH}"
    echo "    Added ${TAURI_BIN_DIR} to PATH"
fi

echo
# Link binary names to standard command names if needed
if [ -x "${TAURI_TARGET}" ]; then
    if ! command -v cargo-tauri >/dev/null 2>&1; then
        ln -sf "${TAURI_TARGET}" "${TAURI_BIN_DIR}/cargo-tauri" 2>/dev/null || true
    fi
fi

echo "==> Verifying pkg-config stacks…"
pkg-config --modversion webkit2gtk-4.1 || echo "WARN: webkit2gtk-4.1 not found"
pkg-config --modversion gtk+-3.0 || echo "WARN: gtk+-3.0 not found"

echo
echo "Setup complete. Build the desktop client with:"
echo "  export PATH=\"\${HOME}/.local/bin:\${PATH}\"  # if not already in .bashrc/.zshrc"
echo "  cd crates/wl-app && cargo tauri dev"
