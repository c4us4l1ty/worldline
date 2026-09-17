## License & Copyright

**Copyright (c) 2026 c4us4l1ty. All Rights Reserved.**

*This repository is strictly proprietary. You may 
NOT use, modify, sub-license, or redistribute this code for any purpose without permission.*

# Worldline

A local-first, zero-knowledge execution terminal. The system plans (Stackelberg
leader); you execute one non-negotiable directive at a time (follower). No
emails, no accounts, no lists — a 12-word BIP-39 mnemonic is the only identity.

## Architecture

```
Worldline/
├── crates/
│   ├── wl-core/       Pure-Rust core: crypto identity, HLC, SQLite store,
│   │                  CRDT LWW engine, Tier-3 heuristic engine, AI dispatch
│   ├── wl-protocol/   Wire types shared by client & relay
│   ├── wl-relay/      Axum blind-blob relay (Ed25519 challenge auth,
│   │                  opaque ciphertext routing; sqlite dev / postgres prod)
│   ├── wl-sync/       Offline outbox drain + pull/apply + deterministic merge
│   └── wl-app/        Tauri v2 desktop shell (420×747, non-maximizable)
│       └── ui/        Dioxus (wasm) frontend — the 9:16 single-directive canvas
```

**Identity (PRD §3):** 128-bit OS entropy → 12-word BIP-39 mnemonic →
HKDF-SHA256 with versioned labels → Ed25519 signing key (relay auth; pubkey
hex = Account ID) + ChaCha20-Poly1305 payload key (E2EE of every sync op).
The relay stores only opaque ciphertext it can never decrypt.

**Sync (US-4):** every local write lands in SQLite with HLC metadata and an
encrypted op in the durable outbox. Reconnection drains the outbox, pulls
remote ops after a cursor, decrypts, and merges deterministically (LWW with
device-id tie-break + tombstones). Convergence is property-tested.

**AI (PRD §4, BYOK):** Tier-1 Master Architect (goal → milestones), Tier-2
Tactical Dispatcher (daily 1–3 directives, 48h context window), Tier-3 local
Rust engine (offline, timers, phases, escape-hatch reactions, velocity EWMA).
OpenAI-compatible + Anthropic adapters; model ids are user settings. Manual
goal creation works with no API key at all.

## Build & run

### Core, relay, sync (any machine)
```bash
cargo test --workspace        # 80 tests: crypto vectors, HLC, CRDT convergence,
                              # engine invariants, relay auth/pull, two-device sync
cargo run -p wl-relay         # blind relay on 127.0.0.1:8080 (SQLite backend)
```

### Desktop client (needs Tauri's Linux deps once)
```bash
bash scripts/setup-linux.sh   # sudo apt webkit/gtk headers + cargo install tauri-cli, dioxus-cli
cd crates/wl-app
cargo tauri dev               # 420×747 window, Alt+Space summon, dark graphite theme
cargo tauri build             # bundle .deb/.AppImage
```

### Relay, Postgres backend (production parity)
```bash
docker compose up -d          # Postgres on :5433
cargo run -p wl-relay --no-default-features --features postgres \
    # with WL_RELAY_PG_URL=postgres://worldline:worldline@localhost:5433/worldline_relay
```

### UI in a browser (mock harness, no shell needed)
```bash
cd crates/wl-app/ui && dx serve   # mock command layer enables full UI development
```

## Development workflow (low-CPU)

First builds are heavy (Tauri pulls ~600 crates); everything after is
incremental. Rules of thumb:

```bash
export CARGO_BUILD_JOBS=4   # cap parallel codegen; add to your shell profile

# UI-only work: browser mock, zero Rust recompiles (~0 idle CPU)
cd crates/wl-app/ui && dx serve

# Full app, iterative: capped compile, no file watchers
CARGO_BUILD_JOBS=4 cargo tauri dev --no-watch   # run from crates/wl-app

# Full app, daily use: build once, run the binary (no watchers, no dev server)
cargo tauri build --no-bundle                   # from crates/wl-app
./crates/wl-app/target/release/wl-app

# Relay: already in target/ — run the binary directly, no cargo overhead
./target/debug/wl-relay                         # :8080, SQLite backend; ~0 idle CPU
```

There is no JavaScript toolchain in this repo (no node, no bun — the UI is
Rust/Dioxus compiled to wasm). All commands above are cargo/tauri/dx.

## Design system

UI follows `.opencode/skills/worldline/SKILL.md`: matte graphite `#131312`
canvas, `#20201F` directive card, cream `#DAD5C7` CTA, coral `#E26D52` timer
beacon, DM Sans / Doppio One / SF Mono typography, zero-guilt velocity
treatment. Exactly one directive is rendered at any moment. ⌘+Enter completes;
Escape opens the frictionful bailout modal (categorize: blocked / scope /
energy). No streaks, no backlog views, no alarm red.

## Environment

- Noise: none. No telemetry, no accounts, no identifiers.
- Secrets: mnemonic + API keys are process/Stronghold-resident; they never
  enter SQLite, the DOM, or the relay.
- PRD deltas are logged in `docs/PRD-DELTAS.md`.

## Verification status (CI-free repo; commands)

| Scope              | Command                                            | Status |
|--------------------|----------------------------------------------------|--------|
| Core/relay/sync    | `cargo test --workspace`                            | 80 pass |
| Lints              | `cargo clippy --workspace`                         | clean  |
| Formatting         | `cargo fmt --all --check`                          | clean  |
| UI wasm            | `cd crates/wl-app/ui && cargo build --release --target wasm32-unknown-unknown` | clean |
| Desktop shell      | `cargo test/clippy --manifest-path crates/wl-app/Cargo.toml` | 5 pass, clean (live-relay handshake + vault round-trip tests) |
| Desktop bundle     | `cd crates/wl-app && cargo tauri build` (after setup script) | by user |
