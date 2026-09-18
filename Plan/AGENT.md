# AGENTS.md — Autonomous Agent Steering Protocol

## Core Principles
1. **Self-Healing:** If a build, compile, or test step fails, read the error output, modify the relevant code, and retry execution automatically (up to 3 attempts per issue).
2. **Task Isolation:** Scope compiler and test checks to specific crates/packages before running project-wide checks to prevent background task timeouts.

---

## Workspace Setup & Context
- **Project Type:** Rust / Tauri v2 desktop shell + Dioxus (wasm) frontend — no JS toolchain (no node/bun)
- **Key Crates:** `wl-core`, `wl-protocol`, `wl-relay`, `wl-sync` (root workspace) · `wl-app` (Tauri v2 shell, EXCLUDED from workspace — needs webkit/gtk headers, has no lib target) · `wl-ui` (`crates/wl-app/ui`, Dioxus wasm, standalone workspace)
- **Primary Tooling:** Cargo, Tauri CLI, dx (Dioxus CLI)

---

## Execution Pipeline

### Phase 1: Baseline Verification
- Run fast checks synchronously. Do NOT background these commands.
  ```bash
  export CARGO_BUILD_JOBS=4
  cargo check -p wl-core
  cargo test --workspace            # 162 tests
  cargo clippy --workspace --all-targets   # must be warning-free
  cargo fmt --all -- --check
  # Shell (standalone workspace; needs webkit/gtk via scripts/setup-linux.sh;
  #  skip in restricted environments, cover logic via wl-core tests instead):
  cargo test --manifest-path crates/wl-app/Cargo.toml      # 15 tests
  cargo clippy --manifest-path crates/wl-app/Cargo.toml
  # UI (standalone workspace, Rust/Dioxus→wasm):
  cd crates/wl-app/ui && cargo test -p wl-ui               # 10 tests
  cd crates/wl-app/ui && cargo check --target wasm32-unknown-unknown
  # Total: 187 tests green (162 workspace + 15 shell + 10 UI).
  ```
- Notes: `wl-app` is excluded from the root workspace (`Cargo.toml:15`
  `exclude`), so `cargo check -p wl-app --lib` never resolves — always use
  `--manifest-path crates/wl-app/Cargo.toml`. The UI is Rust/Dioxus→wasm,
  not a Node/Webview frontend.
