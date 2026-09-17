# AGENTS.md — Autonomous Agent Steering Protocol

## Core Principles
1. **Self-Healing:** If a build, compile, or test step fails, read the error output, modify the relevant code, and retry execution automatically (up to 3 attempts per issue).
2. **Task Isolation:** Scope compiler and test checks to specific crates/packages before running project-wide checks to prevent background task timeouts.

---

## Workspace Setup & Context
- **Project Type:** Rust / Tauri Application
- **Key Crates:** `wl-app`, `wl-core`, `wl-relay`
- **Primary Tooling:** Cargo, WASM tools, Node/Webview frontend

---

## Execution Pipeline

### Phase 1: Baseline Verification
- Run fast checks synchronously. Do NOT background these commands.
  ```bash
  cargo check -p wl-core --lib
  cargo check -p wl-app --lib
