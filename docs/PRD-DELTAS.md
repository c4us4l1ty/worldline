# PRD Deltas — deviations from the Product Requirements Document

Locked decisions from the planning session are marked [approved].

## Architecture

1. **UI framework: Dioxus** [approved] — PRD offered "Leptos or Dioxus";
   Dioxus chosen for dx hot-reload + Tauri IPC fit.
2. **Multi-device key model: single root key** [approved] — all devices
   derive identical keys from the mnemonic; "pairing" = re-entering the 12
   words. No per-device keys in v1.
3. **X25519 dropped from the E2EE path** — the PRD lists "X25519/ChaCha20"
   but payload encryption with a single root key needs only symmetric
   ChaCha20-Poly1305 (HKDF-derived). X25519 becomes relevant only with
   per-device keys (v2); the protocol can adopt it without breaking changes.
4. **Session tokens** — PRD says "PASETO / JWT". v0.1 uses opaque random
   bearer ids with server-side session state (equivalent security for a
   single-cluster relay; revocable). PASETO v4.local wrapping can be added
   behind the same `Authorization: Bearer` header without protocol change.
5. **AI model names not hardcoded** — PRD names GPT-4o / Claude 3.5 Sonnet /
   Haiku / Gemini Flash. Tier-1/Tier-2 model ids are user-configurable BYOK
   settings (`app_settings.tier1_model`, `tier2_model`), provider adapters:
   OpenAI-compatible + Anthropic [approved].
6. **Manual goal-creation fallback** [approved] — without an API key (or
   offline), users hand-author goals/milestones/directives locally; the AI
   restructures plans once a key exists.
7. **Relay in monorepo, sqlx dual-backend** [approved] — SQLite for dev/tests
   (zero infrastructure), Postgres for production. The relay stores only
   opaque blobs either way, so the zero-knowledge contract holds.
8. **Desktop-first; mobile-ready** [approved] — no Android/iOS scaffolding in
   v1 (sandbox lacks toolchains); wl-core is platform-clean so
   `tauri android/ios init` is drop-in later.
9. **Fonts bundled as TTF** (not woff2/CDN) — webview-compatible,
   local-first; conversion tooling was unavailable in the build sandbox.

## Schema (PRD §7)

10. **`identity_config.created_at INTEGER` → `hlc_timestamp TEXT`** — PRD's
    own §7 header says "HLC Unix epoch" while every other table uses TEXT
    `pt.ctr.device`; standardized on TEXT everywhere ("HLC everywhere").
11. **Added tables the PRD describes in prose but omits from SQL**:
    - `goals` (root of milestone tree; velocity anchor: Δ remaining
      milestones / Δ remaining days — PRD §5.4 formula),
    - `directive_phases` (progressive micro-directives, PRD §5.2),
    - `check_ins` (evening audit, PRD §5.4),
    - `bailouts` (escape-hatch ledger with reason enum, PRD §5.3),
    - `app_settings` (non-secret config; BYOK keys stay in Stronghold),
    - `crdt_applied` (pull idempotence watermark),
    - `hlc_clock` / `sync_cursor` (clock + cursor persistence).
12. **`crdt_outbox` extended** with `created_at_epoch_ms` (queue ordering)
    and `pushed` flag (durable drain state).
13. **`directives.progressive_total` added** — PRD has `progressive_step`
    but no total; phases need both.
14. **`milestones` gained `goal_id`** — PRD's milestones had no parent.

## UX / behavior

15. **Bailout scope-downsizing: floor at 15 minutes** — PRD says "halve";
    unbounded halving of small tasks produces noise. 20 min → 15 (floor).
16. **Estimate recalibration floor 60%** — velocity-driven downsizing never
    shrinks future estimates below 60% of the original (avoid compounding
    under-estimation in the other direction).
17. **Default hotkey `Alt+Space`** as PRD states, configurable in Settings;
    noted collision with Linux window-manager menus (WM config expected to
    release the combo, or the user rebinds).
18. **Verification challenge indices** — 3-word verification now uses
    shell-issued random indices per onboarding (was deterministic in v0.1);
    the UI falls back to fixed positions only when the shell omits them.
19. **Vault key is a device-local `0600` file, not the OS keychain** —
    `crates/wl-app/src/vault.rs` stores the Stronghold snapshot key at
    `0600` in the app data dir. Same threat envelope as the SQLite DB
    itself; OS-keychain migration is tracked future work (the `Vault::open`
    boundary is already shaped for it). No secrets ever touch SQLite/DOM.
20. **Snapshot KDF work factor is 0** — iota-Stronghold's file encryption
    defaults to a multi-second password-KDF per commit; our snapshot key
    is a 32-byte CSPRNG secret (not a password), so stretching is pure
    latency. Upstream documents 0 as correct for strong keys.
21. **tauri-plugin-stronghold NOT registered as a plugin** — its JS-side
    commands would expose a second, zero-keyed vault to the webview. The
    shell manages its own `Vault` instance; the crate stays as a dependency
    for the snapshot format + `Stronghold` type.

## Toolchain

19. **Desktop shell excluded from workspace** — `crates/wl-app` needs
    webkit2gtk dev headers; CI-less sandbox builds stay green without them.
    `scripts/setup-linux.sh` installs prerequisites; the shell compiles on
    any normal Linux dev machine.
20. **In-process transport for convergence tests** — US-4's "<300 ms" is
    asserted on the merge path (excluding real network RTT) in
    `wl-sync/tests/convergence.rs`.

## Sync hardening (battle-test pass, 2026-09-15)

21. **HLC text is fixed-width `pt(20).ctr(5).dev(5)`** — unpadded decimal
    broke TEXT ordering at every digit boundary (9→10), inverting relay
    delivery order; fixed width makes every `WHERE hlc > ?` / `ORDER BY
    hlc` / LWW guard sort identically to numeric HLC order (C4).
22. **Pull cursor is the composite `(hlc, operation_id)`** — strict `hlc >
    since` permanently skipped same-HLC ops; ties now advance via the op
    id and the cursor persists in `sync_cursor` after every batch (C2+C3).
23. **Push drains the whole outbox per cycle** — one batch per "Sync now"
    click stranded offline bursts; the push phase loops until drained or
    no progress, and both `accepted` + `duplicates` mark ops pushed so a
    lost push response never wedges an op pending forever (C5+P3).
24. **Pull applies under LWW arbitration (incl. `app_settings`)** — the
    wire path previously blind-upserted by arrival order; every table now
    guards with `hlc_timestamp < op.hlc` (migration 0003 adds the missing
    column to the singleton settings row) (C1).
25. **HLC head persists in `hlc_clock`; counter saturates, never wraps** —
    restarts under a regressed wall clock used to re-issue older
    timestamps (LWW inversion); the head is now read at boot and written
    on every tick, and counter exhaustion steps physical forward (E5+E6).
26. **Relay registration + challenge flood caps** — unauthenticated account
    minting is capped (10 000 accounts → 429) and in-flight challenges are
    bounded per-account (4) and globally (100 000); sessions GC on sight
    in `validate` with a throttled sweep (S3+S5).
27. **Relay URL allow-list** — `relay_url` must be a bare http(s) base, no
    credentials/query/fragment, no link-local metadata hosts; enforced in
    `settings_save`, `relay_authenticate`, and `sync_now` (S4).
28. **Payload key zeroized on drop** — `Identity.payload_key` is
    `Zeroizing<[u8; 32]>` (the doc comment previously claimed drop glue
    the type never had) (S1).
29. **Backup verification is positional, server-of-record is the shell** —
    challenge indices persist in `identity_config.verify_indices` and the
    shell re-checks `verify_backup_words`; caller-supplied indices are
    honored only for pre-persistence legacy rows, and `identity_unlock`
    preserves (never wipes) the stored indices (S2).
30. **Read-path activation write-throughs** — `current_directive` passes
    the unlocked identity into the engine so canvas-load activation emits
    an outbox op like every other transition; the one-active-directive
    invariant now holds across devices (E4).
31. **Downsize resets + rescales phases** — scope-downsize requeues at
    phase 1 with phase minutes rescaled to sum to the new estimate (was:
    resumed mid-phase with rows totaling the old estimate); final-phase
    completion marks the phase `done` before closing the directive, and
    `complete()` reports post-advance phase minutes (E1+E2+E3).
32. **No shell `hud-tick` timer** — the 1 Hz Tauri event emitter had no
    listener (the UI owns its own second loop); removed to eliminate a
    permanent wake-up source toward the near-0-CPU goal (P1).
33. **Known limitations kept as characterization guards, not fixed here**:
    relay push holds one transaction/mutex across the batch (P4 —
    `defect_push_holds_global_mutex_across_whole_batch`); velocity
    `estimate_adjustment` is computed and displayed but not applied to
    future estimates (E7); `blocked` directives have no unblock path and
    `skipped` never completes a milestone (E8).
