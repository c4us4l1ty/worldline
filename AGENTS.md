# AGENTS.md — Worldline working conventions

## Commands

```bash
cargo test --workspace        # THE verification gate (216 tests; 283 total with shell+UI)
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

### The shell's frontend is chosen at COMPILE time

Tauri picks its frontend source from the `custom-protocol` Cargo feature:

| Build | Feature | Loads | Needs a server? |
|---|---|---|---|
| `cargo tauri build` | ON | embedded `frontendDist` bundle | no |
| `cargo tauri dev` | OFF | `devUrl` = `http://localhost:1420` | yes (it starts one) |
| plain `cargo build` / `cargo run` | OFF | `devUrl` = `http://localhost:1420` | **yes, and nothing starts it** |

**A plain `cargo build` + `target/debug/wl-app` with no dev server on 1420
shows "Could not connect to localhost: Connection refused" INSIDE the
Tauri window** — that is the *browser's* error page, not a broken app. The
process is alive and healthy. `build.rs` prints a `cargo:warning` naming
this whenever the feature is off; do not run the debug binary directly
unless `dx serve --port 1420` is already up.

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
- **Tauri arg shapes: every command carries `rename_all = "snake_case"`.**
  `tauri-macros` defaults to `ArgumentCase::Camel`, so without it the
  shell looks for `targetDate` while the UI (plain serde structs, no
  `rename_all`) sends `target_date`. A required param then fails with
  `missing required key`; an `Option<T>` param fails **silently** —
  `create_goal` bound without a deadline and the compose screen's
  horizon picker looked like it worked. Snake_case is what the Rust
  params and `invoke-shim.js` already spoke, so the shell is where the
  conversion belongs. Keep the attribute on new commands; `src/ipc_tests.rs`
  drives the real `generate_handler!` over a mock runtime and will fail if it
  is dropped.
  Struct params must still be wrapped (e.g. `{"settings": {...}}`); flat
  objects fail deserialization. The browser mock in `invoke-shim.js`
  ignores shapes — the shell test is the gate, not `dx serve`.
- `#[tauri::command]` fns must be `pub(crate)` (cross-module `generate_handler`).
  A command taking `AppHandle` must be generic over `R: tauri::Runtime`,
  not the `Wry` default, or the mock-runtime test app cannot register it.
- Bearer tokens live only in `AppState.relay_token` (memory); re-handshake
  on HTTP 401 and retry once (`sync_now`). Session TTL is 1h server-side.

## UI boot invariants (2026-09-27 — the app showed two frozen splashes)

- **The wasm module can execute more than once.** `dx serve`'s
  hot-reload re-imports it, and a debug build loads `devUrl`, so a
  second execution used to `launch(App)` a second component tree into
  the same `#main`: two roots, every shell command issued twice, and
  two sets of tasks writing into two different copies of app state.
  `app::main()` now claims the mount point first and returns without
  launching if it is already claimed. **Do not remove the guard** — and
  if you ever mount the app somewhere else, claim it there too.
- **Boot is bounded, always.** `BOOT_TIMEOUT_MS` (15 s) arms a
  watchdog before anything is awaited; the worst case is
  `BootError` + Retry, never an unbounded splash. Its bounds are
  `const _: () = assert!(…)` invariants, not tests.
- **Boot steps are independent.** `settings_get` and `identity_status`
  run concurrently. They used to be sequential, so one hung call meant
  the other was never even attempted.
- **Retry is a state change, not a remount.** It bumps
  `ctx.boot_attempt`; the boot effect keys off that counter.
- **`AppCtx` signals are owned by `ScopeId::APP` explicitly.** They are
  `Copy` and written from detached `spawn`ed tasks, which run at the
  runtime root scope — Dioxus warns about this on every build. The
  warning is a false positive *because* the owner is the root scope, so
  do not "fix" it by inheriting a nearer scope, and do not restructure
  the signals without re-checking ownership.
- **Every screen renders inside an `ErrorBoundary`.** A panic during
  render otherwise leaves the last good DOM in place, which for a
  boot-time panic means the splash stays forever and reads as "still
  starting".
- **`transport()` reports which IPC path resolved** (`native` /
  `mock` / `none`) and is shown on the boot-failure screen. A window
  running on the mock looks identical to a working one from outside,
  and nothing you do is being saved.

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
  (beacon ONLY, never a surface; the session timer that used to be the
  headline use was removed 2026-09-26). Pure `#000`/`#FFF` prohibited.
- Fonts: DM Sans (directives/CTA), Doppio One (brief greeting), ui-monospace
  (HUD/telemetry/seed words). Bundled TTFs in `ui/assets/fonts` — no CDN.
- Never render a list of future tasks, streak counters, or red failure states.
  Skips are "velocity adjustments" (GPS metaphor).
- Escape hatch requires reason categorization before the directive unmounts.
- Secrets (mnemonic, BYOK keys) never touch the DOM or SQLite.
