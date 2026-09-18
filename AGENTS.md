# AGENTS.md — Worldline working conventions

## Commands

```bash
cargo test --workspace        # THE verification gate (162 tests; 187 total with shell+UI)
cargo clippy --workspace      # must be warning-free
cargo fmt --all               # run before committing
```

- Desktop shell (`crates/wl-app`) is EXCLUDED from the workspace because it
  needs webkit/gtk dev headers. Do NOT add it to workspace members. Verify it
  only after `scripts/setup-linux.sh`; in restricted environments check logic
  via `wl-core` tests instead.
  Shell gates (run from repo root):
  `cargo test/clippy --manifest-path crates/wl-app/Cargo.toml` (must be
  warning-free) + `cargo fmt --manifest-path crates/wl-app/Cargo.toml`.
  Includes a live-relay handshake test (spins up `wl-relay` on ephemeral TCP).
- UI crate: `cd crates/wl-app/ui && cargo check --target wasm32-unknown-unknown`.
  It is also standalone (own `[workspace]` table) — keep it that way.

## Dev workflow (low-CPU)

- `export CARGO_BUILD_JOBS=4` — cap parallel codegen on shared machines.
- UI-only: `cd crates/wl-app/ui && dx serve` (browser mock, no recompiles).
- Iterative app: `CARGO_BUILD_JOBS=4 cargo tauri dev --no-watch` (from `crates/wl-app`).
- Daily use: `cargo tauri build --no-bundle`, then run
  `crates/wl-app/target/release/wl-app` directly.
- Relay: `./target/debug/wl-relay` (already built; ~0 idle CPU).
- No JS toolchain exists here (no node/bun) — UI is Rust/Dioxus→wasm; all
  commands are cargo/tauri/dx.

## Shell conventions (hard-won — do not regress)

- `Repos.conn` is `Mutex<Connection>` (rusqlite is `Send` but not `Sync`).
  All locks are statement-scoped; never hold a guard across a nested `Repos`
  call (`prepare` sites bind a local guard; verified deadlock-free).
  This is what makes `AppState: Send + Sync` for Tauri state.
- Sync I/O (`relay::handshake`, `ReqwestTransport::post`) drives nested
  runtimes via block_on → may ONLY run on `spawn_blocking` threads; calling
  from async code panics. `sync_now`/`relay_authenticate` already wrap.
- Tauri arg shapes: top-level JSON keys must match command param names.
  Struct params must be wrapped (e.g. `{"settings": {...}}`); flat objects
  fail deserialization. The browser mock in `invoke-shim.js` ignores shapes —
  verify new commands against the real shell, not just `dx serve`.
- `#[tauri::command]` fns must be `pub(crate)` (cross-module `generate_handler`).
- Bearer tokens live only in `AppState.relay_token` (memory); re-handshake
  on HTTP 401 and retry once (`sync_now`). Session TTL is 1h server-side.

## Architecture rules

- **wl-core is platform-clean**: no tokio, no Tauri, no I/O beyond SQLite. New
  logic goes here with unit tests; the shell stays a thin command layer.
- **Zero-knowledge discipline**: the relay (`wl-relay`) must NEVER see
  plaintext. Anything crossing the wire flows through
  `wl-core::crypto::aead::seal` with `table:record` AAD.
- **Stackelberg invariant**: at most ONE directive is ever `active`. The
  engine (`wl-core::engine`) enforces this — never bypass it in UI code.
- **HLC everywhere**: every persisted row mutation stamps
  `hlc_timestamp` (TEXT `pt.ctr.device`). No raw epoch columns.
- **PRD deltas**: deviations from the PRD must be recorded in
  `docs/PRD-DELTAS.md`.

## Design system (binding)

All UI code follows `.opencode/skills/worldline/SKILL.md`. Highlights:
- Canvas `#131312`, card `#20201F`, CTA `#DAD5C7`, coral accent `#E26D52`
  (timer/beacon ONLY, never a surface). Pure `#000`/`#FFF` prohibited.
- Fonts: DM Sans (directives/CTA), Doppio One (brief greeting), ui-monospace
  (HUD/timers/seed words). Bundled TTFs in `ui/assets/fonts` — no CDN.
- Never render a list of future tasks, streak counters, or red failure states.
  Skips are "velocity adjustments" (GPS metaphor).
- Escape hatch requires reason categorization before the directive unmounts.
- Secrets (mnemonic, BYOK keys) never touch the DOM or SQLite.
