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

## Frontend pass (2026-09-16, UI-crate only — no new shell commands)

34. **BYOK onboarding allows skip** — spec shows `BIP39 → BYOK → runtime`
    as a hard gate; the UI routes verify/restore → `ByokSetup` → runtime
    but `Continue without key — manual mode` stays (PRD delta 6, Tier-3
    manual fallback). Returning unlocks (`identity_unlock`) go straight
    to the canvas since provider choice persists in settings.
35. **Morning briefing titles-only** — `morning_briefing` returns
    `Vec<String>`; milestone headers + per-directive estimates on the
    brief screen are labels from `current_directive`/static copy, not
    shell estimates. Estimates render authoritatively on the canvas.
36. **Telemetry drawer is reuse-only** — HLC head and SQLite WAL size have
    no shell command and render as unavailable instead of fabricated
    data; drawer shows `settings_get` state + `sync_now` pending/pushed/
    pulled + static `420×747` geometry. Full key purge stays per-provider
    `delete_api_key` in Settings (no vault-wide purge command added).
37. **Escape modal centered, not bottom-sheet** — spec backdrop is
    `rgba(19,19,18,0.85)` centered; the old bottom-sheet CSS was replaced.
    Backdrop click still asks `Keep the directive active?` (friction
    preserved); `1/2/3` + `Esc` keys and a 140-char note cap added.
38. **`Ctrl+,` is the web equivalent of spec `⌘,`** — WASM sees Ctrl;
    the drawer also opens from the timer pill and closes on `Esc`.
    `Alt+Space` summon stays native in `hotkey.rs`.

## Battle-test pass (2026-09-16, singularity audit — 120 tests)

39. **Blank UI root causes fixed** — `frontendDist` pointed at a
    never-built `release/` dir (now built + verified); asset URLs are
    relative (`./wl.css`, `./invoke-shim.js`); new
    `capabilities/main-capability.json` grants `core:default` to the
    `main` window (explicit `label: main` in `tauri.conf.json`,
    referenced via `app.capabilities`); the shim resolves
    `__TAURI_INTERNALS__.invoke` lazily (eager capture pinned
    native to mocks forever); `bail_out` takes flat
    `reason`/`note` params (was a `req` wrapper the UI never sent —
    Escape hatch always failed); dropped the unused `router` feature;
    `dx build --release` output (incl. `<title>`) verified.
40. **Poison-op quarantine** — a pulled op that fails base64 / sealed
    / unseal / JSON / HLC checks is watermarked in `crdt_applied`
    and skipped (`SyncStats.quarantined`) instead of aborting before
    the cursor advance (one crafted row used to brick all future
    pulls). Newer-schema enum strings from future clients are
    quarantined the same way (CHECK constraints reject them
    server-side of the merge).
41. **HLC receive events on pull** — every applied remote op merges
    into `Repos.hlc` via `observe_remote_hlc` (+ persisted head);
    `observe_remote` previously built a throwaway clock (causality
    no-op). `wall_nanos` degrades to 0 instead of panicking on a
    pre-1970 clock (head + counter path still monotonic).
42. **Single-active self-healing** — `set_directive_state(Active)`
    parks any other active to queued (own tick + outbox
    write-through); `enforce_single_active` reconciles merged
    dual-actives after pull (newest HLC wins, deterministic tie-break
    `min id`), called from `sync_cycle` when `applied > 0`.
43. **Sync loop bounds** — push/pull paginate at most 200 batches per
    cycle; `batch_limit` clamped to 1–500 (a 0 limit hot-looped empty
    pulls; a hostile `exhausted=false` relay looped forever).
44. **Outbox drain is transactional** — `mark_outbox_pushed` commits
    one transaction (was N statements; a crash left half-drained).
45. **Row-mapper panics removed** — corrupt enum strings return
    `BadEnum` mapping errors (layer 2; layer 1 is the schema CHECK
    constraints, which refuse such writes outright — covered by test).
46. **`set_mnemonic_verified` param indices fixed** — was `?2/?3`
    with two params (every verification write errored).
47. **AI phase-sum band** — `validate_directive` rejects phase tables
    outside 2× of the headline estimate (the sum replaces the
    estimate at persist time; 60m→10m silent rewrites impossible).
48. **Relay push boundary** — per-op validation (canonical fixed-width
    HLC round-trip, 7-table allow-list, id lengths, 256 KiB sealed
    cap), 500-ops-per-request cap (413), 100k-ops-per-account quota
    (429, enforced in-txn on SQLite / under table lock on Postgres).
    500s are generic ("storage failure", detail in server log).
49. **Relay no longer blocks/panics the executor** — all store calls
    run via `spawn_blocking` (SQLite mutex off async workers;
    Postgres `block_on` off the runtime — it panicked under axum).
    Postgres `register_account` cap is now atomic (`LOCK TABLE`).
    Postgres-only build fixed (ungated sqlite re-export/import).
50. **SSRF blocklist extended** — 100.100.100.200, 0.0.0.0,
    IPv4-mapped IPv6 metadata forms, GCE metadata hostname;
    loopback/RFC1918/ULA stay allowed (default local relay + LAN
    self-host). DNS-rebinding TOCTOU documented (impact capped by
    E2EE blindness).
51. **API-key Zeroizing end-to-end** — `ProviderAdapter` holds
    `Zeroizing<String>` (moved, not `.to_string()`-cloned, from the
    vault guard); hand-rolled redacted `Debug` (derived Debug leaked
    keys into any `{:?}` path). Residual: header strings + HTTP
    client internals (short-lived, documented).
52. **Vault file hygiene** — snapshot pre-created `0600` before
    Stronghold opens it (no world-readable window); pre-existing key
    files with lax modes tightened to `0600`.
53. **Idle CPU** — HUD 1 Hz tick gated on the Canvas screen (zero
    wake-ups on onboarding/settings/dormant); `sync_status` is
    `Signal<String>` (was `Box::leak` per sync); one process-wide
    reqwest client (was per-RPC `Client::new` + TLS rebuild);
    unused `tower-http` dep removed. `operation_id` global-uniqueness
    squat and `mark_outbox` mutex convoy stay as documented
    characterization guards (squat needs UUID prediction —
    infeasible; convoy is throughput-only).
54. **Mnemonic DOM hygiene** — `generated` + verify inputs cleared on
    verify success; restore textarea cleared on restore success (the
    single mandated display remains an accepted exception to
    "secrets never touch the DOM").

## Battle-test pass 2 (2026-09-17, singularity audit — 162 tests)

55. **Identity vault ordering** — `identity_generate`/`identity_restore`
    now persist the mnemonic to the vault BEFORE writing public SQLite
    config (previously a snapshot-write failure published an account
    with no recoverable secret). `Vault::save_mnemonic` restores the
    previous live record on commit failure; replacement of an existing
    account is now rejected explicitly. Regression:
    `identity_snapshot_failure_preserves_memory_disk_and_public_state`.
56. **Transactional core mutations** — `create_goal`, `create_milestone`,
    `create_directive`, `set_milestone_status`, `reschedule_directive`,
    `advance_progressive_step`, `upsert_check_in`, `record_bailout`,
    `save_settings` commit row(s) + encrypted outbox op + HLC head in
    ONE transaction using the row's exact HLC (was autocommit + separate
    emit → permanent unsynced divergence on failure). Phases atomic;
    nonpositive durations rejected; missing milestone now NotFound.
57. **Relay auth hardening** — one-shot challenge consumption (parallel
    verify cannot double-mint), expiry now `<= now`, canonical lowercase
    64-hex public keys only, session-quota → HTTP 429, honest token docs.
58. **Per-account dedup** — relay operation_id uniqueness scoped to
    `(account_id, operation_id)` in BOTH stores (SQLite idempotent PK
    rebuild, Postgres DO-block swap): a cross-account id collision no
    longer silently drops the second account's op. Regression:
    `operation_id_collisions_stay_scoped_per_account`.
59. **Pull cursor integrity** — an empty pull batch must restate the
    exact cursor or the cycle aborts before advancing (no silent data
    skips); per-op pull bounds (sealed size, table allow-list) match the
    push side.
60. **Crypto hygiene** — BIP39 seed, HKDF output and AEAD payload keys
    held in `Zeroizing`/borrowed views; migration timestamps degrade
    pre-epoch clocks to 0 and saturate overflow (no startup panic);
    adversarial test header corrected.
61. **Blank-window root causes closed** — release build hook now pins
    `dx build --release --debug-symbols false` (wasm-opt SIGABRT shipped
    a 3.7 MB debug wasm instead of 0.8 MB optimized); `build.rs` FAILS
    the shell build when the embedded frontend dist is missing (a plain
    cargo build previously booted a silently blank window); stale
    bundle leftovers no longer accumulate after clean rebuilds; setup
    paints the webview background `#131312` so boot never flashes white.
62. **App identity serialization** — generate/restore/unlock serialize
    on the identity mutex; invalid persisted device ids fail closed
    instead of silently rotating the CRDT device identifier;
    IPC restore/API-key inputs wrapped in `Zeroizing`.
63. **Verification gates** — 147 workspace + 15 wl-app tests pass,
    clippy 0 warnings (workspace + wl-app), rustfmt clean,
    `scripts/smoke-native.sh` pixel-asserts the release window
    (stddev 104, canvas 0.26 on KDE/Wayland); idle release binary
    measured at 2 ticks / 20 s CPU (0.01%) and 151 MiB RSS.
    Known deferred (documented, not fixed): HLC arrival-sequence cutover,
    AEAD AAD operation_id/HLC binding, AI batch atomicity, vault API-key
    rollback, spawn_blocking refactor for sync IPC.

## Phase-1 fixes (2026-09-19 — B-001…B-009)

73. **Provider matrix pinned to four OpenAI-compatible endpoints** —
    `openrouter | google | qwen | bytez.com`. The Anthropic-native
    adapter variant is removed; a single `OpenAiCompat` variant plus a
    per-provider base URL covers the whole matrix. Endpoints: OpenRouter
    `https://openrouter.ai/api/v1`, Google
    `https://generativelanguage.googleapis.com/v1beta/openai`, Qwen
    `https://dashscope.aliyuncs.com/compatible-mode/v1`, bytez
    `https://api.bytez.com/models/v2/openai/v1` (per bytez OpenAI-compat
    docs). `base_for` returns `None` (reject) instead of silently
    falling back to OpenAI; the shell allow-list is pinned equal to
    `KNOWN_PROVIDERS` by test; the vault provider-id charset gains `.`
    for `bytez.com`.

74. **Manual goal auto-seeds its starter set (B-002 option b)** — shell
    `create_goal` seeds milestone "First steps" + one 25-minute
    directive titled from the goal, scheduled today, so
    `Engine::current` activates it on the next canvas load with NO API
    key and NO unlocked identity. Core `Repos::create_goal` is
    unchanged (AI `persist_plan` must not inherit seeds). Directive
    authoring UI was deferred to Phase-2 MVP-1, and the
    `create_manual_milestone` / `create_manual_directive` shell
    commands existed for it (both deleted in delta 157 — the compose
    screen never called them; `create_goal` does the whole job).

75. **`estimate_adjustment` is observed, not applied (MVP-3 honesty)** —
    `engine::velocity::compute` returns `adjustment = 0.6 + 0.4·ratio`
    (floored 0.6) but no writer consumes it: future estimates ignore it
    (CORE-2/E7 deferred). The check-in screen now labels it
    "observed adjustment … (not yet applied to future estimates)"
    instead of claiming "Tomorrow's estimates reflect actual velocity".

76. **Morning briefing exposes authored-vs-persisted split (B-005)** —
    dispatcher returns `BriefingResult.created_ids` from
    `persist_briefing`; shell returns `BriefingView { titles,
    created_ids }`; UI re-fetches `current_directive` + `velocity`,
    runs one `sync_now` surfacing `pushed/pulled/pending` in the HUD,
    and shows a GoalCreate CTA when nothing persisted — never a silent
    blank canvas.

## Battle-test pass 3 (2026-09-18, singularity audit — 162 workspace + 15 shell + 10 UI tests)

64. **Write-boundary budgets** — every local write path now rejects
    blank/oversized text (titles ≤ 500 chars, descriptions/context ≤
    4000, notes ≤ 2000, model ids ≤ 256), strict `YYYY-MM-DD` calendar
    dates (chrono alone accepts `2026-9-8`, corrupting lexicographic
    date ordering), and directive estimates within 1–1440 min; bailout
    notes truncate to the spec 140-char cap; settings enforce
    `dark|light`, a 64-char hotkey (empty normalizes to `alt+space`),
    and a 4-provider allow-list; AI facades validate before the
    billable HTTP hop. Motivation: unbounded strings bloat sealed ops
    past the relay 256 KiB cap and wedge sync with an undrainable batch.
65. **LWW arbitration is total** — `TableState.last_op` carries a
    tertiary `operation_id` tie-break (identical full-HLC ops from
    forked data dirs previously converged order-dependently); strict-`<`
    SQL guards stay sound because boot jitter (below) makes full-HLC
    equality unreachable across live replicas.
66. **Clone-split boot jitter** — `Repos::new` advances a restored HLC
    head by a random 0–255 counter (persisted on next tick): two live
    replicas forked from one data dir no longer stamp identical HLCs
    on different content under a regressed wall clock (permanent fork).
67. **Constraint conflicts quarantine** — SQLite UNIQUE/FK failures in
    pull-apply watermark-and-skip instead of aborting the cycle (a
    same-id/different-date row move used to wedge every future pull);
    other store errors still abort. Regression:
    `poison_constraint_op_quarantines_without_wedging_sync`.
68. **Bounded sync metadata** — relay-durable outbox rows are deleted
    after the push phase and `crdt_applied` watermarks at/below the
    persisted cursor are pruned each cycle (both tables grew forever).
    Migration 0004 repairs the v2/v3 `'0'` HLC backfills to canonical
    zero (bare `'0'` fails row mapping on legacy rows).
69. **Shared HTTP client** — one process-wide reqwest client for
    sync/handshake/AI (connection reuse; per-call nested runtimes
    unchanged for the block_on discipline). Relay `now_secs` degrades
    a pre-1970 clock to 0 instead of panicking.
70. **Dead code removed** — `IdentityVault` (unimplemented Stronghold
    variant), `EngineError::NotActive`, `Repos::set_phase_state_lww`,
    `wl_sync::observe_remote`, shim `wlOnHudTick`/`wlIsNative`
    (the latter's web fallback ran a `setInterval` nothing consumed);
    `enqueue_outbox` validates table/record bounds.
71. **UI correctness** — BYOK/settings key probes re-subscribe on
    provider switch with stale-response guards (pill switch showed the
    wrong "sealed" state); pin-to-top persists via `settings_save`
    and reverts on failure (was ephemeral until restart); vault-key
    removal UI added (`delete_api_key` was shell-only); goal creation
    double-submit guarded on both paths with title checks; briefing
    distinguishes missing-key from offline and counts actual
    dispatches; browser mock verifies backup words positionally like
    the shell; `active_directive` orders by `(hlc DESC, id)` matching
    the `enforce_single_active` winner.
72. **Release dist prunes stale hashed assets** — `dx build`
    content-hashes wasm/js but never deletes superseded bundles, so
    every rebuild permanently added ~800 KiB of dead weight that Tauri
    embeds into the shipped app. `scripts/prune-dx-dist.sh` (wired into
    `beforeBuildCommand` via `scripts/build-ui.sh`) keeps exactly the
    assets reachable from `index.html` (js directly, wasm via the kept
    js); public assets, fonts, and the shim are never touched.

## Audit pass 2026-09-26 (verification-only; no behavior change)

77. **Two user-facing security claims were false and are now corrected** —
    a doc-truth audit against the dependency source found that the app
    asserted protections it does not have. No behavior changed; only copy
    and doc comments.
    - **"Keys never touch the DOM"** (`settings.rs`, `byok.rs`) was
      untrue: the key is bound into an `<input value>`, and a webview
      cannot receive typed input any other way. A user-typed secret
      necessarily transits the DOM. The claim is replaced with the
      guarantee that actually holds — sealed into the vault, cleared
      from the field after save, never written to SQLite, never sent to
      the relay. `AGENTS.md`'s equivalent binding rule carries the same
      impossible premise and is left for an owner decision
      (`Plan/Plan.md` AUDIT-4).
    - **"Hardware keychain (Stronghold) · enclave locked · Argon2id ·
      128-bit salt"** (`telemetry.rs`) was false on all three counts.
      Verified against `iota_stronghold-2.1.0`: the snapshot cipher is
      **XChaCha20-Poly1305** (`src/internal/provider.rs:7`); there is
      **no Argon2id anywhere in the dependency tree**; the snapshot KDF
      work factor is deliberately **0** (see #20 — the key is a 32-byte
      CSPRNG secret, not a password); and `Vault::open` constructs
      Stronghold with the key and **never calls `lock()`**, so "enclave
      locked" was false too. The vault is a local encrypted file at
      `0600` — not hardware-backed, not an OS keychain (#19 / SHELL-6).
      The telemetry drawer now states the real properties and discloses
      the OS-keychain gap in-product rather than implying protection
      that does not exist. `vault.rs`'s module header carried the same
      two errors and was corrected to match.

## Implementation pass 2026-09-26 (SHELL-1, CORE-3…6, SYNC-1/3/4, SHIP-2/3)

78. **Mutex poisoning recovers instead of failing the process** — the
    single highest-severity fix in this pass. 65 sites across
    `wl-core`, `wl-sync`, `wl-relay` and `wl-app` called
    `lock().unwrap()`/`.expect(..)`, so one panicking writer made a
    `Mutex` permanently unusable: the app, the relay, or the HLC (which
    every row mutation touches) could never take it again, stranding
    the user's data dir with no recovery path. New `wl-core::poison`
    provides `LockRecover::lock_recover()` for plain state and
    `lock_conn()` for the SQLite handle, which additionally rolls back a
    half-open transaction left by a panicking writer. Poison is
    deliberately **not** propagated as an error: a `Mutex<Connection>`
    cannot manufacture a fresh connection, so every call site would
    need a restart path for a condition strictly worse than continuing.
    14 of the 65 sites were in the long-lived relay or the HLC hot path.

79. **CRDT reject is now side-effect free** — `TableState::apply`
    inserted the merge head and cleared the tombstone *before* checking
    that a field-less upsert had any fields, then returned `false`. A
    poison op therefore corrupted the merge memory and resurrected a
    deleted row without writing it back, and the head advance made the
    next *legitimate* op lose as stale. Validation is hoisted above all
    mutation.

80. **`identity_config` is a singleton** — migration `0006`. The old
    `public_key` primary key let a restore with a different phrase append
    a second identity, so `identity()`'s `LIMIT 1` was an arbitrary
    pick and `set_mnemonic_verified` (a bare `UPDATE`) stamped the flag
    onto every row. Now `id = 1 CHECK (id = 1)`, with duplicates
    collapsed to the earliest row so an upgrading install keeps the
    identity it already trusted.

81. **Goals have a lifecycle; the plan no longer orphans the old goal** —
    `GoalStatus` had no writer, so every goal stayed `active` forever and
    `active_goal()` broke ties arbitrarily. Added `set_goal_status` +
    `archive_other_active_goals`; `persist_plan` archives the superseded
    goal, so the single-active invariant that already held for
    directives now holds for goals.

82. **A progressive directive with missing phase rows self-heals** — the
    old `Invalid("current phase missing")` broke `activate_next`,
    `complete` and the HUD with no way forward. `Repos::ensure_phases`
    rebuilds the rows as `progressive_total` equal slices of
    `estimated_minutes` (remainder to the last step, so the sum is exact)
    through the normal write path, so the repair *replicates* rather than
    silently diverging. Genuinely unsplittable directives still fail
    closed, but the error names the remedy.

83. **Relay push commits in chunks** — the store has one write mutex, so a
    single transaction per batch convoyed every concurrent pull. Now
    `INSERT_CHUNK_SIZE = 128`, chosen above `MAX_BATCH_OPS` so an
    ordinary client push still commits once. The per-account quota is
    re-checked inside *every* chunk against a live `COUNT`; crossing the
    cap retains the committed prefix rather than discarding
    acknowledged work (ops are idempotent, so a retry reports duplicates).

84. **Relay feature selection is a build error** — `--no-default-features`
    previously failed with a baffling `E0425: cannot find value 'blobs'`,
    and `--features sqlite,postgres` silently selected SQLite while the
    operator believed they had deployed Postgres. Both are now
    `compile_error!`s that say what to type. Postgres also gained
    migration `0002`: its pull index lacked the `operation_id` tail, so
    every pull sorted the account's whole tail — a latency cliff the
    SQLite dev path never showed.

85. **The pull size budget is coherent and enforced** — the caps were
    mutually inconsistent as a product (`500 ops × ~341 KiB` base64
    implies a ~167 MiB *legitimate* response), which is why nothing was
    enforced: `MAX_RESPONSE_BYTES` was referenced only by a text
    validator and `MAX_PULL_BYTES` was dead code. `MAX_PULL_BYTES` (8 MiB)
    is now authoritative, `MAX_RESPONSE_BYTES` is defined as it, the
    relay stops filling a batch at the budget (cursor taken from the last
    op *included*, so pagination stays lossless), and the client reads
    with the same constant instead of a hand-picked 16 MiB.

86. **`save_settings` validates `relay_url` at the write boundary** — this
    closes a live SSRF path, not a tidiness gap: `save_settings` is also
    reached by pull-apply, so a peer could replicate an `app_settings`
    row carrying a `file://` or metadata-endpoint URL that the next sync
    cycle would dial, bypassing the shell's pre-validation entirely.

87. **An oversized CRDT op is rejected, not enqueued** — the relay 400s
    any sealed op over 256 KiB, which does not merely reject it but makes
    it *undrainable*: it sits in the outbox forever, every drain fails
    the whole batch, and sync is wedged with no local remedy. The cap is
    now enforced at enqueue, so the caller's row write rolls back in the
    same transaction and the store never holds state it cannot replicate.

88. **Supply chain is configured and green** — `deny.toml` did not exist,
    so the `cargo audit`/`cargo deny` clause was unconfigured rather than
    merely unpassed. Configuring it found two real problems: rustls
    0.23.44 was vulnerable to RUSTSEC-2026-0285 (TLS 1.3 handshake
    messages accepted across encryption-level boundaries; fixed by
    0.23.45), and `bincode` was declared in two manifests while being
    referenced by zero source files — removed rather than kept.

89. **`workspace.package.license` is a real SPDX identifier** — was the
    crates.io `UNLICENSED` placeholder, which is unparseable and made
    `cargo deny` hard-fail all four first-party crates as `unlicensed`.
    Now `LicenseRef-Proprietary-AllRightsReserved` (owner-approved);
    same proprietary intent, but expressible to tooling.

90. **The wasm size budget is a real guard** — `prune-dx-dist.sh` is a
    staleness pruner and enforces no bytes; the plan had been treating it
    as a size gate. New `scripts/check-dist-size.sh` fails the release
    build when the wasm payload exceeds 1 MiB (current: 840 KB, 80% of
    budget) and is wired into `build-ui.sh`. Verified in both directions.

## Canvas chrome pass (2026-09-26 — user-directed)

91. **The canvas header bar is gone; controls float over the canvas.**
    A full-width HUD strip (`<header class="wl-hud">`) sat above the
    canvas carrying six elements: menu, an Evening audit nudge, the
    milestone label, sync status, a sync-now button, and the timer. It
    fought the product's own premise — one directive, no chrome — and
    squeezed the empty state into a letterbox. Removed per `Plan/2.png`
    → `Plan/3.png`; the menu is now a floating circular control with a
    soft drop shadow, matching the reference.

    **The timer deliberately stayed on the canvas.** It is the coral
    beacon and the only live-updating element; a focus timer you cannot
    see is not a timer. Milestone label, sync status and the Evening
    audit nudge moved to the telemetry drawer (`Ctrl+,`), which already
    rendered sync stats and an always-visible Evening audit button — so
    nothing became unreachable and B-006's guarantee still holds. Sync
    status now has its own "Current line" block in that drawer.

    Implementation notes worth keeping: the layer is `pointer-events:
    none` with the controls opting back in, so it cannot intercept clicks
    meant for the directive card beneath it; `.wl-directive-container`
    gained `padding-top: 62px` to reserve the controls' band; `.wl-hud`
    is retained only as a generic inline row for `byok.rs`, and its
    former `border-bottom` must not be restored. The dead `.wl-hud-timer`
    and `.wl-hud-meta` rules were deleted rather than left behind.

    `.opencode/skills/worldline/SKILL.md` §4.A was rewritten to match —
    it is a binding doc, and leaving it specifying the old `<header>`
    would have invited the next agent to undo this.
92. **The session timer is gone from the product.** Removed entirely
    (2026-09-26, user directive) after it had just been re-homed as a
    floating pill. Gone with it: the `MM:SS` readout, the 1 Hz
    `start_timer` JS ticker and its `TimerSubscription` teardown, the
    `TimerSession` type and its `update()` lifecycle, the
    `timer_session` signal maintained on every `set_directive`, and
    `fmt_mmss`. The canvas now shows **no elapsed time at all** — work is
    bounded by the directive, not by a clock, which is a defensible
    reading of "one directive, no chrome" and removes the only source of
    per-second re-render in the app.

    `elapsed_secs` was **kept**: it is not timer-shaped, it backs the
    sync-freshness label (`SYNCED · 42s ago`) in the telemetry drawer.
    Its test was renamed and rewritten to say so, and a sub-second
    truncation case was added in place of the two `fmt_mmss` assertions
    it lost.

    Consequences recorded rather than hidden:
    - `Ctrl+,` is now the **only** way into the telemetry drawer; the
      timer used to be a second entry point. The keybinding is unchanged
      and independent.
    - Sync age no longer refreshes on a timer-driven re-render. It is
      computed at render time and the drawer re-renders on open, so the
      displayed age is still correct when read.
    - `.wl-hud-timer` and `.wl-float-timer` CSS were deleted, not left
      as dead rules.
    - The coral accent lost its headline justification. `#E26D52` is now
      carried by the active step badge and the create-goal button, so the
      "beacon only, never a surface" rule still has real load-bearing
      uses. The skill's colour table and AGENTS.md were corrected to stop
      promising a timer that no longer exists.

## Page redesign pass (2026-09-26 — user-directed, visual only)

93. **Goal creation and Settings redesigned around a shared page shell.**
    Visual/markup only: every handler, shell command, payload, and
    validation string was carried over verbatim (verified — the entire
    pre-`rsx!` region of `settings.rs` is byte-identical to its previous
    form, and both `goal_create.rs` closures are unchanged).

    **A real bug was found and fixed underneath the "the design is not
    good" complaint: goal creation had NO way back.** The only exit was
    completing a goal, so opening it and changing your mind was a dead
    end. Both pages now open with the same fixed header carrying a
    circular back affordance (`←`, `aria-label="Back to the line"`)
    that returns to the canvas.

    - The header sits **outside** the scroll region with
      `flex-shrink: 0`, because on a long page (settings) an in-flow
      back button scrolls away exactly when it is needed. Verified by
      rendering a scrolled state: the header and back button remain.
    - Settings previously buried "Back to the line" at the very bottom
      styled `.wl-btn-escape` — the *bailout* affordance — so ordinary
      navigation read as a destructive action. It is now navigation,
      where the eye expects it.
    - Eight undifferentiated fields became four labelled sections
      (Identity / Appearance / Intelligence / Sync). Goal creation's
      four loose fields became "The objective" and "Boundaries".
    - "Create manually — no API key" was `.wl-btn-escape` for the same
      reason; it is now `.wl-btn-ghost`. An ordinary fallback no longer
      looks destructive.
    - `settings_save` writes **every** field, so "Save settings" was
      deliberately kept outside any section (with a note saying so)
      rather than reading as belonging to Sync.
    - Minor: the ad-hoc `wl-hud-pill` reuse for the "sealed" chip became
      `.wl-chip`; the redundant "Pin to top" label became "Window"; the
      `(YYYY-MM-DD)` hint was dropped from the date field because
      `type="date"` already renders that format.

## Settings & goal-create redesign + hotkey removal (2026-09-27 — user-directed)

94. **Page titles are centred; the back affordance floats.**
    The previous pass gave both pages a back button as the FIRST FLEX
    CHILD of `.wl-page-head`, which pushed the title permanently right of
    the viewport axis — no `justify-content` could centre the text
    without also centring the button. The button is now
    `position: absolute` over the header's left gutter, and the header
    carries symmetric `40px` side padding so the title sits on the true
    centre axis while the control keeps its reserved space.
    `.wl-page-title` dropped 26px → 24px because the centred column is
    40px narrower per side and "State the Objective" is the longest
    title; verified rendering on one line at 420px.

95. **The theme selector is a switch, and the window pin stopped
    pretending to be a button.**
    The theme was a `<select>` listing two options; the pin was a
    full-width button whose label described the state it would put you
    in. Both are boolean device preferences, so both are now
    `.wl-switch[role=switch]` with `aria-checked` and a sliding knob.
    This is why the pin changed too: leaving one as a
    label-bearing button beside a real switch would have read as an
    inconsistency, not a hierarchy.

    **The switches apply AND persist immediately; the text fields still
    wait for "Save settings".** A theme toggle that only took effect on
    Save is a broken control — you cannot evaluate a theme you are not
    allowed to see. This is the one deliberate two-path persistence
    model on the page, and it also closes the previously-open defect
    where the theme applied on change but diverged from the store if
    the user abandoned the page.

    **The switch state colours are absolute, not theme-derived.** A
    switch encodes "which theme is selected", not "what is currently
    rendered", so flipping its tokens with `data-theme` would invert the
    whole control the instant it took effect. Verified both switch
    states render with usable contrast in BOTH themes.

96. **A persistence bug the switch model would have introduced, caught
    before it shipped.** `settings_save` writes EVERY field from the
    page's local draft. The switches therefore persist from
    `ctx.settings` — the last shell-confirmed state — with their own one
    field flipped, never from the draft. Reading the draft would have
    meant that flipping the theme silently committed whatever the user
    had half-typed into an unrelated field (a relay URL, a model id).
    Extracted as the pure `switch_payload(shell, draft, field)` and
    pinned by `switch_persists_from_shell_state_not_the_local_draft`;
    the unused `draft` parameter is deliberate, so a future edit that
    starts reading it has to change the signature.

97. **Settings re-reads the shell on mount.** The draft was a mount-time
    snapshot of `ctx.settings` that never re-invoked `settings_get`, so
    the page could display values the store no longer held — and "Save
    settings" would then write those stale values back over newer ones.
    It now re-reads on mount. The trade is explicit: unsaved edits are
    discarded when you navigate away and back, which is the honest
    behaviour for a settings page and strictly better than silently
    resurrecting a snapshot.

98. **The sync control moved from the canvas to Settings.** It was a
    34px circle opposite the menu that reported nothing until tapped.
    Sync is a settings concern, not a canvas one, so it is now a
    full-width row in the Sync section with the last cycle's result
    beneath it — a place where it can show what it DID, not only offer
    to do something. Canvas chrome is now menu-only, which is what the
    single-directive premise wants. Nothing became unreachable: sync
    status and the Evening audit were already in the Ctrl+, telemetry
    drawer. **Consequence: the canvas has no sync affordance at all.**

99. **Icons are inline SVG, not font glyphs.** `☰` and `⚙` were replaced
    with `path` geometry in a new `icons.rs`; `←` became a chevron for
    consistency. This is not cosmetic — a glyph's weight, spacing, and
    (on many Linux desktops) its colour are chosen by the font stack,
    which is where the drawer's settings-icon colour and size drift came
    from. Every icon strokes with `currentColor`, so it inherits the
    active theme tokens and the button's own `colour` for free.
    Sizing is CSS-only (`.wl-icon` + a per-icon modifier) so a new icon
    cannot arrive at an arbitrary size, and the whole set is
    `aria-hidden` because each sits inside a control that already
    carries the real `aria-label`.

    **The create-goal `✚` was deliberately left as a glyph**: a plus is
    unambiguous at 20px and never suffered the rendering drift. Only the
    two that did were replaced.

    The moon/sun knob icons are on a 24-unit grid rather than a
    hand-rolled 16-unit crescent. The 16-unit version was geometrically
    correct but at a 12px render box its terminator and outer arc
    collapsed into an unreadable blob — verified by rendering and
    zooming, not by reading the path.

100. **The global summon hotkey was removed as a feature.** Deleted:
    `tauri-plugin-global-shortcut` (dependency + plugin registration),
    `crates/wl-app/src/hotkey.rs`, the `toggle_window_visibility`
    command (which existed only to serve it), the `AppSettings.hotkey`
    field in `wl-core`, its write-boundary validation, the `settings()`
    SELECT column, the `settings_json` CRDT payload key, the
    `wl-sync` pull-apply column, the UI field, the telemetry line, and
    both browser-shim mocks. The B-007 hotkey-validation fix went with
    it.

    **No DB migration.** `app_settings.hotkey` is
    `NOT NULL DEFAULT 'alt+space'`, so once the column left the INSERT
    statement SQLite supplies the default. Dropping a column on a
    CRDT-synced singleton costs a migration and buys nothing — no reader
    ever asks for it. The column is now orphaned by design, and both
    `repo.rs` and `wl-sync/sync.rs` say so at the statement.

    **Sync stays compatible in both directions.** A new peer's payload
    has no `hotkey` key; an OLD peer's payload that still carries one is
    ignored rather than rejected. The `app_settings` upsert simply
    stopped reading it.

    **Consequence: there is no global summon.** The window is reachable
    through the taskbar (`skipTaskbar: false`); there is no tray icon.

101. **Two latent CSS bugs fixed in passing.** `.wl-float-btn`
    transitioned on `var(--duration-fast)`, a token defined nowhere in
    the stylesheet — the declaration was invalid, so the floating
    controls' hover/active animation silently collapsed to `0s` and they
    snapped instead of easing. And `.wl-switch-knob` needed
    `overflow: hidden`, not defensively but because the crescent moon
    was being shaved by the knob's own `border-radius` into a blob.

102. **Two design-token drifts corrected.** The nav drawer's app name
    was `--wl-text-primary` where the spec says cream `#DAD5C7` (a linen
    wordmark read as body text rather than a logotype), and the settings
    pill was `--wl-surface-elevated` where the spec says
    `--wl-surface-card` — which would have made the button invisible
    against the drawer it sits on, so it was moved to
    `--wl-surface-high` (one step above the sheet) rather than obeyed.

103. **Test counts.** 13 UI tests (was 10): three added for the switch
    payload rule, the flip direction, and theme-toggle symmetry from an
    unrecognised starting value. Workspace 187, shell 17.

104. **Page sub-headings removed; titles are one plain line.**
    Both secondary pages dropped their deck beneath the title
    ("The architect builds the milestone hierarchy…" /
    "Secrets stay in the local vault…"). Under a two-word heading they
    restated the heading rather than adding information, and they cost
    ~40px of vertical space on a page that has to scroll. `.wl-page-sub`
    was deleted with them — it had no other users.

    Removing them is also what let the header be genuinely centred: the
    title plus a deck is a two-line block whose optical centre sits below
    the axis, so a single line is both simpler and better centred.

105. **The goal-creation title is now "State the Objective", in the same
    plain style as "Settings".** It previously accented the word
    "objective" in coral italic (`wl-italic-accent`), which was wrong
    twice over. It made that one heading read as a different kind of
    object from every other screen, and it spent the coral beacon — a
    live-status accent — on a decorative word. The beacon's sanctioned
    uses are the active step badge, the create-goal button, and
    cryptographic security badges.

    `wl-italic-accent` itself is unchanged and still correct for the
    *editorial* headings it was designed for: the morning greeting, the
    dormant line, the evening recalibration, and the seed-phrase title.
    This is a correction of where the accent belongs, not a removal of
    the accent. (It also listed the BYOK screen, which has since been
    removed as unreachable — see delta 119.)

    Verified by rendering: the goal-creation form now fits one 420×747
    screen with both CTAs visible, which the deck had been pushing below
    the fold.

## Compose screen + gear icon (2026-09-27 — user-directed, from Plan/1.png)

106. **Goal creation became a compose screen: one free-text field that
    owns the page.** Title, details, and constraints fields are gone.
    The reference (`Plan/1.png`) is a bare canvas with one large text
    field, and the screen now follows it: a top bar with the back
    chevron and a target-date horizon pill, the centred title beneath,
    the field owning the middle, and a side-by-side commit row.

    The field is deliberately bare — no card, no resting border —
    because a box around it fights the "this IS the page" reading.
    Affordance comes from the caret, a muted placeholder, and a coral
    hairline that exists only while focused, which is also the only
    coral on the screen.

    **`Enter` inserts a newline; only the buttons submit.** There is no
    keyboard shortcut, deliberately: the field is multi-line, so Enter
    must stay a newline, and a submit chord on a compose screen invites
    firing it mid-thought. The two buttons are the only commit path.

107. **The architect now names the goal — a real contract change, not a
    UI change.** The old Tier-1 contract had no title in the response at
    all: `tier1_user` sent `GOAL: {title}` and the schema was
    `{"milestones":[…]}`, so the title always came from the caller.
    Feeding it a raw fragment would have named the goal "in n out
    burger" forever. So:

    - `PlanResult` gained `title` and `description`, both
      `#[serde(default)]` so a response omitting them still parses.
    - `tier1_user(intent, target_date)` collapsed GOAL/DETAILS/CONTEXT
      into one `USER INTENT` block, and the schema now leads with
      `title`. The prompt says the intent may be a fragment, a
      paragraph, or contain constraints — otherwise a model that
      expected a tidy one-line GOAL field plans against the whole
      paragraph verbatim.
    - `AiDispatcher::master_plan` dropped `goal_desc` and `context` for
      a single `intent`, bounded by `MAX_DESCRIPTION_CHARS` (the store's
      own prose budget). The now-dead `MAX_AI_CONTEXT_CHARS` constant
      went with it.
    - `master_plan`'s Tauri args changed from `goal_title` /
      `goal_description` / `context` to `intent`. Arg names ARE the wire
      contract, so `goal_create.rs`, `invoke-shim.js`, and the
      four-provider test moved in the same change — and the shim cannot
      catch a mismatch, since it ignores arg shapes entirely.

108. **Title precedence: the model wins, the intent is the fallback.**
    `persist_plan` prefers `plan.title` when non-blank, else
    `fallback_title(intent)` (first non-empty line, clipped to
    `MAX_TITLE_CHARS` on a word boundary), else `"Untitled goal"`.

    Falling back rather than failing is deliberate: a model that returns
    a perfectly good plan but no title must not cost the user a billed
    call. A title that is *present but blank* is treated as absent, not
    as an error, for the same reason. Both are pinned by tests, because
    the failure mode is silent — an "Untitled goal" looks like a bug
    report, not like a missing optional field.

109. **Dropping the constraints field is a real capability loss,
    recorded rather than absorbed.** Structured constraints (hours/day,
    skills, deadlines) were a distinct input from intent; the architect
    no longer has a dedicated slot and must infer them from prose inside
    the intent. That is the right trade for a one-field compose screen,
    but it is a trade, so it is written down here instead of being left
    to look like an oversight.

110. **The manual path splits the same text client-side.** `create_goal`
    is unchanged: the UI sends the first non-empty line as `title` and
    the remainder as `description`.

    The 80-character clip on that title is load-bearing.
    `seed_first_steps` creates the first directive with the goal title
    VERBATIM, so an unclipped paragraph would become the directive
    headline on the 420px canvas. The goal title is otherwise invisible
    in the UI — `create_goal`'s response is parsed as
    `serde_json::Value` and discarded, and no screen renders it — so
    this is the one place it surfaces.

    A blank remainder is sent as `None`, not `Some("")`, so the store
    writes NULL rather than an empty string.

111. **Target date is a horizon pill, not a date input.** Six options
    (No date / 1 week / 1 month / 3 months / 6 months / 1 year)
    resolving to a date at selection time. "By when?" is a horizon
    question far more often than a calendar one, and this keeps the
    control a pill that matches the reference's language; a native date
    widget would have read as a different object from everything else on
    the page. It is a real `<select>` restyled, so keyboard and
    screen-reader behaviour are the platform's.

    `horizon_date` returns `None` rather than a malformed string when
    the arithmetic is not representable — the store's `check_date` is
    shape-exact and would reject "" or "2026-9-8" at the write
    boundary, after the call was already made.

112. **`.wl-page-bar` exists because the title and the horizon pill
    cannot share a row.** The title is ~230px wide inside a 304px
    padded box, so an absolutely-positioned pill in the right gutter
    overlays its last ~50px. Splitting the bar from the title removes
    that arithmetic instead of tuning around it.

    The bar also has to reset `.wl-back` to `position: static`. The
    chevron is absolutely positioned in `.wl-page-head` to overlay a
    centred title; left absolute inside the bar it leaves the flow, the
    pill becomes the bar's only in-flow child, and `space-between` pins
    it to the LEFT edge. Found by rendering, not by reading the CSS.

113. **The settings icon is a gear, rebuilt from measured geometry.**
    The `⚙` glyph was replaced by SVG, then by a sliders mark that read
    as "audio mixer" rather than "preferences", and now by a real gear.

    It is a single filled path with `fill-rule="evenodd"`: the outer
    sub-path is the notched rim and the trailing circle sub-path is the
    hub hole, which evenodd knocks out. That is why this one icon is
    `fill: currentColor` while the rest are stroked.

    Proportions were measured off the reference mark, not guessed: outer
    r = 8.6 on a 24-unit grid, hub hole r = 3.4 (≈0.40 of the outer
    radius), root r = 7.0, eight teeth at ±15°. Two earlier attempts
    were rendered and rejected — a stroked ring-plus-teeth version read
    as a **sun**, and a first filled version with deeper, wider notches
    read as a **ship's wheel**. Both are recorded here because the tuned
    values look arbitrary otherwise.

114. **Two bugs the new tests caught, both mine, both silent.**
    `split_intent` used `skip_while(|l| !l.trim().is_empty())` to drop
    the title line — but that predicate is TRUE for non-empty lines, so
    it skipped the title and kept the leading blanks, discarding the
    whole description. And the first `fallback_title` clip could return
    an empty title when the first "word" was longer than the budget.
    Neither is reachable by reading the code; both are now pinned.

## Build-wiring bugs found while verifying the app (2026-09-27)

115. **`beforeBuildCommand` / `beforeDevCommand` pointed one directory too
    high, so `cargo tauri build` could never run.** Both were
    `../../scripts/…` with `"cwd": "ui"`, which resolves to
    `crates/scripts/…`. The scripts live at the repo root, so from
    `crates/wl-app/ui` the correct prefix is `../../../`. Every
    `cargo tauri build` died at `beforeBuildCommand … exit code 127`,
    which means the documented daily-use flow in both README and
    AGENTS.md had never actually worked. Fixed to `../../../scripts/…`.

    Worth noting how this stayed invisible: the same broken prefix also
    appears nowhere else, `build-ui.sh` resolves its own root from
    `BASH_SOURCE`, and the *debug* path (`cargo build`, which does not run
    either hook) appeared to work — it just loaded the dev server URL, so
    it failed differently.

116. **A plain `cargo build` produces a binary that cannot show the app.**
    Tauri chooses its frontend source at COMPILE time from the
    `custom-protocol` Cargo feature: ON → the embedded `frontendDist`
    bundle; OFF → `devUrl` = `http://localhost:1420`. `cargo tauri build`
    sets the feature; a plain `cargo build` / `cargo run` does not.

    So `crates/wl-app/target/debug/wl-app` run without a dev server on
    1420 renders the **browser's own error page inside the Tauri window**:
    *"Could not connect to localhost: Connection refused"*. The process
    is alive and healthy, which is what makes it so confusing — it reads
    as a broken app rather than a missing server.

    `build.rs` now emits a `cargo:warning` whenever `CARGO_FEATURE_CUSTOM_PROTOCOL`
    is unset, naming the two correct commands. An assert cannot catch
    this one: the bundle really is present and the binary really is
    valid, so the only honest signal is a build-time message. The same
    trap is documented in README and AGENTS.md with a table of which
    build mode needs a server.

## IPC arg binding, dead screen, and boot routing (2026-09-27)

117. **Every command now declares `rename_all = "snake_case"`; the shell
    was silently dropping `target_date`.** `tauri-macros` 2.6.3 defaults to
    `ArgumentCase::Camel` (`command/wrapper.rs:51`), so a bare
    `#[tauri::command]` made the shell resolve arguments against
    `targetDate` while the Dioxus UI — which serialises plain serde structs
    with no `rename_all` — sent `target_date`. Four commands declare
    multi-word parameters: `create_goal`, `create_manual_milestone`,
    `create_manual_directive`, `master_plan`.

    The failure splits in two, and the quiet half is the one that mattered:

    * **Required parameters** (`goal_id`, `milestone_id`,
      `estimated_minutes`, `scheduled_for_date`) failed loudly —
      `CommandItem` uses a strict `payload.get(key)` with no default
      (`tauri/src/ipc/command.rs:100-103`).
    * **`Option` parameters** (`target_date`) failed **silently**:
      `deserialize_option` visits `None` for an absent key
      (`command.rs:135-140`). So `create_goal` bound successfully and
      persisted a goal with **no deadline**, and the compose screen's
      horizon picker was a control that appeared to work and did
      nothing. Same for `master_plan`, so AI-authored goals were
      deadline-less too. This is the failure mode worth remembering:
      a Tauri arg-key mismatch is not necessarily a crash.

    Fixed in the shell rather than the UI, because snake_case is the
    convention the rest of the repo already speaks — the Rust parameter
    names, `invoke-shim.js`, the manual authoring commands — and the
    conversion belongs in one place. Single-word parameters are
    unaffected (`to_snake_case("title") == "title"`), so nothing that
    worked before changed.

118. **The IPC layer had zero test coverage; it now has the only test
    that can see this class of bug.** `crates/wl-app/src/ipc_tests.rs`
    drives the real `generate_handler!` over `tauri::test::MockRuntime`
    (`tauri` gains a `test` dev-feature; upstream `test = []` pulls in no
    new dependencies). Three tests, each asserting the *value* that came
    back rather than merely the absence of an error, because the silent
    `Option` case above produces no error to assert on.

    The handler list is extracted into `add_commands()` in `main.rs` so
    the test builds its mock app over the same list the app registers —
    a hand-copied list would keep passing while the app shipped a
    different one. Making that generic over `R: Runtime` required
    `set_always_on_top` to become `async fn …<R: tauri::Runtime>(app:
    tauri::AppHandle<R>, …)`: `AppHandle<Wry>` does not implement
    `CommandArg<MockRuntime>`, so the `Wry` default pinned the command to
    the shipped runtime and out of the test.

    The regression was confirmed by reverting `rename_all` from
    `create_goal` and watching `target_date` come back `null` — which is
    the whole point: the test catches data loss, not just exceptions.

119. **`Screen::ByokSetup` was unreachable and is deleted.** Nothing ever
    assigned it; the only writer was inside the screen it rendered. Its
    spec role (BYOK key entry) is fully covered by the compose screen's
    AI-or-manual choice plus Settings' Intelligence section, which owns
    provider, key set, and key delete. `screens/byok.rs` and the variant
    are removed rather than left dormant, along with `.wl-hud`,
    `.wl-hud-pill`, `.wl-hud-status` and `.wl-pulse-dot` in `wl.css` —
    those four rules existed only for that screen.

120. **The boot routing decision is extracted; the landing screen is
    unchanged.** `App`'s boot effect now calls
    `app::boot_screen(Result<(), &str>)` instead of matching inline, so
    the fail-closed rule is testable without a DOM: a healthy install
    lands on the Canvas, and a shell that refused to answer renders
    `BootError` carrying the shell's own message, rather than falling
    through to a default screen that would disguise a broken vault as a
    fresh install.

    Routing boot at the Morning Brief instead was proposed — the brief
    is the screen that states the day's trajectory, and with the compose
    screen in place it had no entry point from the canvas or the nav
    drawer, only the unadvertised chain `Ctrl+,` → evening check-in →
    dormant → brief — and was **declined on 2026-09-27**: the Canvas is
    the surface that should open unprompted. So the brief's
    reachability is left as it was, and is recorded here as an open
    product question rather than a fixed defect.

121. **`wl-core`'s sealed-op cap is now pinned to the relay's at compile
    time.** `MAX_SEALED_OP_BYTES` was a hand-mirrored literal of
    `wl_protocol::MAX_SEALED_BYTES`, annotated "the two must move
    together" — which a comment cannot enforce. If the mirror drifts low,
    `wl-core` writes an op the relay rejects, the outbox can never drain,
    and sync wedges with no failing test. `wl-sync` depends on both
    crates, so it now holds
    `const _: () = assert!(wl_core::…MAX_SEALED_OP_BYTES == wl_protocol::MAX_SEALED_BYTES)`.
    No new dependency: the assertion lives in the crate that would
    actually break.

122. **Test counts.** Workspace 192, shell 20, UI 19 (231 total). The
    three shell IPC tests are new; the rest of the delta from the
    previous 217 is the compose screen's own tests.

## Boot hardening: two frozen splashes (2026-09-27, user-reported)

The app opened to two copies of the boot splash that never resolved.
Three independent defects could each produce that picture, and the
design had no way to distinguish them, so all three are closed.

123. **The app mounted twice.** `dx serve`'s hot-reload re-imports the
    wasm module, and a debug build loads `devUrl`, so a second module
    execution ran `launch(App)` a second time into the same `#main`.
    Reproduced in a browser against `:1420`: every boot command fired
    twice, 384 ms apart. Two component trees meant two sets of `AppCtx`
    signals, and each tree's writes notified only its own subscribers —
    so one tree could sit on `Screen::Boot` permanently while the other
    was perfectly healthy.

    `app::main()` now claims the mount point (clearing any stale tree)
    and returns without launching if it is already claimed, making
    execution idempotent. Verified by re-importing the real release
    bundle: root and card counts are unchanged.

124. **Boot had no failure state, so a wedge looked like patience.**
    The splash read "Establishing cryptographic session…" forever with
    no timeout, no error, and no exit. `BOOT_TIMEOUT_MS` (15 s) now arms
    a watchdog *before* anything is awaited, and the worst case is
    `BootError` with a working Retry. Verified by making
    `identity_status` never settle: at 5 s the splash says so, at 15 s
    the failure screen appears, and clicking Retry after un-wedging the
    shell lands on the canvas with no relaunch.

    The splash also counts elapsed seconds. A static "starting…" is
    indistinguishable from a frozen one, which is how a wedged shell
    presented as a busy app.

125. **One hung boot call blocked the entire boot.** `settings_get` and
    `identity_status` were awaited in sequence, so a `settings_get` that
    never settled meant `identity_status` was never attempted. They now
    run concurrently and independently; routing waits only on identity,
    and a settings failure is tolerated as it always was.

126. **A render panic left stale DOM with no explanation.** A panic in a
    component's render leaves the last good DOM in place, which for a
    boot-time panic is the splash, forever. Every screen now renders
    inside an `ErrorBoundary` with a fallback that names the error and
    offers a reload.

127. **`AppCtx` signal ownership is now explicit and documented.**
    Dioxus warned on every build: *"A Copy Value created in ScopeId(3)
    … used in ScopeId(0) … may cause reads or writes to fail."* That is
    correct in general — the signals are `Copy` and are written from
    detached `spawn`ed tasks, which run at the runtime root scope. It is
    a false positive here only because the owning scope is the app root,
    which outlives every writer. All fourteen are now constructed with
    `Signal::new_in_scope(…, ScopeId::APP)` so the guarantee is stated
    rather than inherited: the refactor that would actually introduce
    the bug is moving this construction into a component with a
    shorter-lived scope, and that must now fail loudly in review.

128. **The app can now say which IPC path it resolved.**
    `app::transport()` reports `native` / `mock` / `none` and is shown
    on the boot-failure screen. `invoke-shim.js` falls back to the mock
    harness whenever `__TAURI_INTERNALS__` is absent, so a window can
    render a fully working-looking app on canned data with nothing being
    saved — previously indistinguishable from a healthy install.

129. **The boot budget is a compile-time invariant.**
    `BOOT_TIMEOUT_MS` is a constant, so asserting its bounds at runtime
    is a tautology. They are now `const _: () = assert!(…)` checks beside
    the constant, the same treatment `MAX_SEALED_OP_BYTES` gets, so it is
    impossible to widen the budget into "never" without failing the
    build.

**Not verifiable here.** The WebKit remote inspector
    (`WEBKIT_INSPECTOR_SERVER`) does not accept connections on this
    machine, so the native IPC handshake *inside the Tauri webview* was
    not observed directly. It is covered indirectly: the shell's
    commands are exercised through the real `generate_handler!` in
    `ipc_tests.rs`, and the same UI bundle was driven screen-by-screen
    in a browser.

## Icon weight and compose symmetry pass (2026-09-27, user-directed)

Four things a person looking at the running app objected to, all
presentational. No command, payload, test or store changed.

130. **The settings gear read as a lumpy washer, not a cog.** The
    mark was already `fill: currentColor` — the complaint that it
    "should be filled" was really about *shape*, and the shape was
    wrong in a way that survived every earlier tuning pass because it
    was tuned in isolation. Eight teeth spanned 44° of a 45° pitch, so
    the notches between them were 1° slivers: 0.1px at the rendered
    size, i.e. invisible. A gear whose notches cannot be seen is a
    ring. Compounding it, the path stopped at r=8.6 of its 24-unit
    grid, so it drew 14.3px of actual mark inside a 20px box and lost
    the optical comparison against the bold coral `✚` beside it.

    Rebuilt as a six-tooth cog at 22px: ±13° tips, ±22° roots, tip
    r=10.5, root r=7.6, hub r=3.0. Six teeth is the load-bearing
    choice, not six being rounder — 8 teeth leave ~2.5px each at this
    size and mush, 6 leave ~3.3px, and the gaps come out at 1.95px,
    which is the threshold where the silhouette survives the downscale.
    Real ink is now 19.25px across.

131. **The canvas hamburger was enlarged; the back chevron was not.**
    `.wl-icon-menu` 18px → 20px. The stroke is unchanged at 1.6 on an
    18-unit grid, so the mark also gained weight for free — it now
    lands at ~1.8 physical pixels inside the same 34px circle.
    `.wl-icon-back` stays at 18px: it is a different control in a
    different position, and pairing them would imply they are the same
    affordance.

132. **The empty-state "Open menu" button is gone.** It sat 14px under
    a copy line that already said "Open the menu…", directly below the
    floating hamburger that opens that menu — so the canvas presented
    its one piece of chrome as two, and skill §4 (exactly one floating
    canvas control) was being satisfied only in spirit. The copy line
    now carries the instruction alone. Nothing became unreachable: the
    hamburger is on screen in every canvas state.

133. **The compose field was rendering at 28px because it inherited a
    headline-scale token.** `.wl-compose` used `var(--text-directive)`,
    which is 1.75rem — sized for a directive title on the 420px canvas,
    and the *only* rule in the file that used it (`.wl-directive-title`
    sets its own 24px). In a 384px column that is ~20 characters per
    line, which made a one-sentence intent feel like a wall. Now
    18px/400/-0.01em, line-height 1.5, which sits between
    `--text-body` (15px) and the directive title (24px). The token is
    left in place and documented as a headline scale so the same
    substitution is not made again.

134. **The two commit buttons were asymmetric three ways at once** —
    52px vs 46px tall, 1.35fr vs 1fr wide, and 15px vs 14px type —
    which read as one real button and one leftover. `.wl-actions-row`
    now carries a single rule for both children (`flex: 1 1 0`,
    `height: 50px`, `font-size: 15px`, `font-weight: 600`).

    The equalisation is scoped to that row deliberately.
    `.wl-btn-primary` is 52px because the canvas's commit is a thumb
    target and `.wl-btn-ghost` is 46px everywhere else in the app;
    restyling either class globally would have moved four other screens
    to fix a two-button page.

135. **The secondary commit button stopped nearly disappearing.** At
    `--wl-surface-card` (#20201F) with an 8%-white hairline on a #131312
    canvas, the ghost had almost no contrast against its own page, so
    the pair read as "one button and one smudge" regardless of size.
    Inside this row only, it is now `--wl-surface-elevated` with
    `--wl-border-strong`: the same physical presence as the cream
    primary, without the emphasis. Coral is untouched (skill §1: beacon,
    never a surface).

136. **Three smaller corrections, all to the compose screen.** The
    horizon pill was 11px `--wl-text-muted` and read as *disabled* next
    to the 18px back chevron; it is 12px `--wl-text-secondary`. The
    focus hairline was full-strength coral across the full 384px column,
    which made it the loudest thing on a page built to be quiet; it is
    now `rgba(226,109,82,.5)`. And the title's `text-align`/padding
    were an inline `style` in the rsx, so the page's vertical rhythm
    could not be tuned from the stylesheet where the rest of it lives —
    that is now `.wl-page-title--compose`.

137. **The compose row's secondary needed its own hover state.** Resting
    it on `--wl-surface-elevated` (item 135) put it on exactly the colour
    the global `.wl-btn-ghost:hover` already targets, so hovering the
    secondary would have looked broken. It steps up to
    `--wl-surface-high` inside this row only.

**Test counts.** Unchanged: workspace 192, shell 20, UI 19 (231 total).
This pass adds none, and none should be expected: every change is
presentational, and the shell's `ipc_tests.rs` gate does not apply
because no command or payload changed. What *is* verifiable was checked
directly — the new gear path was rasterised and inspected as a
silhouette before being committed, and the previous pass's screenshot
of the nav drawer is what showed the ink-diameter gap.

**Not verified visually.** No rendered before/after was captured in
this pass: no desktop browser was attached to the session, so the
420×747 result is argued from the geometry and the stylesheet rather
than observed. Re-run `dx serve` (or the release build) and check the
three touched surfaces — the canvas hamburger, the nav drawer's gear
next to the `✚`, and the compose page's field and commit row.

## Provider model catalog, and five ways the AI path was broken (2026-09-27, user-reported)

"I have to type the AI model's name, and if I make a mistake while
typing it doesn't work." That report turned out to be five independent
defects stacked on top of each other, only the last of which was the one
being complained about. All five are fixed here, together with the model
picker the report actually asked for.

138. **The default model ids were fiction.** With nothing configured, the
    compose screen sent `model: "flagship"` and the morning brief sent
    `"haiku-class"`. Neither string is a model on any provider, so every
    request 400'd. The placeholders existed only to fill a non-optional
    field; nothing checked them. Both are gone, and the shell now refuses
    a blank model with `no model selected — choose an architect model for
    <provider> in Settings` before touching the vault or the network.
    The UI refuses locally first, so the message points at the field that
    needs filling instead of arriving as a failed request.

139. **Every AI failure was reported as "OFFLINE".** Both call sites were
    `Err(_) => flash("OFFLINE — USE MANUAL MODE")` — the error was
    discarded, so a 400 (bad model), a 401 (bad key), a 403 and a genuine
    network failure were indistinguishable. The shell now returns the
    message, and the screens render it **inline** rather than in the
    toast: the toast is an uppercase mono pill capped at 90% width, so a
    real diagnosis wrapped into a three-line lozenge. `.wl-form-error` is
    `--wl-text-secondary` and not red (skill §5 has no red failure
    states — a wrong API key is a configuration state, not an emergency).

140. **The provider's explanation of the failure was thrown away.**
    `http_execute` returned `format!("HTTP {}", status)` and never read
    the body. A provider almost always says exactly what is wrong —
    OpenRouter answers `"No endpoints found matching model 'flagship'"` —
    so the body is the diagnosis and the status is only the framing.
    `ai::catalog::provider_error_message(provider, status, body)` now
    builds the sentence: `key rejected`, `rate limited or out of quota`,
    and so on, followed by the provider's own words. It lives in
    `wl-core` specifically because it cannot be tested behind a live
    provider, and it handles the shapes these three actually use
    (`error.message`, a bare `error` string, bytez's `error.unknown`) plus
    an HTML proxy page (tags stripped, `<title>` preferred — the output
    is the text `502 Bad Gateway`, not markup).

    It also carries one provider-specific fact: since **2026-06-19**
    Google rejects unrestricted Gemini API keys, which produces a bare
    403 that means nothing to anyone who has not read that changelog. A
    Google 403 now says *"key not permitted: … — Google now requires an
    API-restricted key"*.

141. **`response_format` was sent unconditionally, and ~12% of models do
    not advertise support for it.** The live OpenRouter catalog was pulled
    and measured: **458 models, 443 of them text-output, and
    `response_format` advertised by 391.** Four (`openrouter/auto`,
    `openrouter/fusion`, `openrouter/pareto-code`,
    `openrouter/bodybuilder`) advertise no parameters at all, and
    `anthropic/claude-sonnet-4` — a flagship — is `rf=False`.

    **Corrected after a live probe.** This was first written here as
    "~12% of models reject it", on the reasoning that an unadvertised
    parameter is a rejected one. A real call on
    `nvidia/nemotron-3.5-lightning:free` (also `rf=False`, zero-priced)
    returned **HTTP 200 with the field present** — OpenRouter tolerated
    it. So the metadata marks *unsupported* parameters, not ones
    guaranteed to 400, and the failure is model-specific rather than
    universal. The fix is still correct and still worth having: it stops
    sending a field the provider has said it does not support. The
    original claim was an inference presented as a measurement, which is
    the wrong way round.

    `ProviderAdapter::OpenAiCompat` now carries `json_mode`, set from
    `catalog::json_mode_for`, and omits the field when the catalog says
    the model rejects it. Dropping it is safe: both prompts already demand
    JSON in prose and the parser strips fences and extracts the first
    balanced object, so the field was a preference, not the contract.
    **The switch only ever turns off on positive evidence.** Google and
    bytez.com publish no such flag, so `None` keeps the historical
    behaviour — this can never make a call that works today fail.

142. **Qwen removed; the provider set is now three.** `KNOWN_PROVIDERS`
    (`openrouter | google | bytez.com`) is the single allow-list that
    `vault_api_key`, `save_settings` and `catalog::base_for` all consult,
    and the shell's own `base_for` table is gone — it delegated, because
    the catalog needs the same base URL to build its `/models` endpoint
    and a provider table in two places is one that drifts.

    The removal had a trap worth recording. `save_settings` **rejects** an
    unknown provider, and `settings_save` writes the whole struct, so
    every existing install with `ai_provider = "qwen"` persisted would
    have been unable to save *any* setting — including a theme toggle —
    until the user changed the one field causing it. Worse, the sync path
    writes `app_settings` with a raw INSERT that bypasses the check, so a
    peer could reintroduce a removed provider at any time. `settings_get`
    now sanitizes on read, and `a_removed_provider_does_not_wedge_settings`
    pins it. Tier model ids are deliberately left alone: they are only
    ever sent to a provider, and clearing them would discard a choice the
    user can still make sense of.

143. **New: `wl-core::ai::catalog` — live discovery, no bundled list.**
    The load-bearing decision is that **there is no static catalog
    anywhere.** A baked-in model list is stale the moment a provider ships
    something, and the entire point of this feature is that a model
    released upstream is selectable without a Worldline release. So:
    * `parse_openrouter` keeps only `output_modalities == ["text"]` —
      the live list contains image and audio-output models
      (`google/gemini-3-pro-image`, `openai/gpt-audio`) that return 400 on
      a chat call, and offering them offers a guaranteed failure.
    * Every field is optional and a malformed entry is *skipped, never
      fatal*. A catalog that hard-fails on an upstream schema change is
      exactly how new models stop appearing; the test for that is
      `a_new_upstream_shape_never_breaks_the_list`.
    * `~`-prefixed router ids (`~deepseek/deepseek-pro-latest`) are kept
      verbatim. Ids are charset- and length-bounded so a hostile endpoint
      cannot inject a newline into a value that reaches SQLite, a log line
      and an HTTP body.
    * Google's OpenAI-compat `/models` **is** implemented and returns
      `"id": "models/gemini-2.5-pro"`, so the prefix is stripped here
      rather than being a rule the caller must remember. The native
      `{"models":[{"name":…}]}` shape is accepted too, in case the compat
      route is ever retired.
    * Google and bytez.com report no modality metadata, so a tight
      non-chat denylist (`embed`, `tts`, `imagen`, …) keeps those out of
      the *display*. It filters nothing about use: a hand-entered id is
      always attempted. Hiding a model that works would be worse than
      showing one that 400s.

144. **The catalog is cached in memory, never in SQLite.** `app_settings`
    is a replicated CRDT table; a catalog cached there would sync to every
    peer on the account and two devices would fight over one device-local
    cache. It is a property of the provider at a point in time, so it lives
    in `AppState` behind a 1-hour TTL, and `list_models(force: true)` —
    wired to a "Refresh models" button — bypasses it, so waiting is never
    the only way to see a new model. All three providers are gated on a
    stored key even though OpenRouter's list happens to be public: one rule
    for the user to learn beats three, and a keyless install should not
    pull 750 KB it cannot use.

145. **Model selection is a searchable sheet, not a dropdown.** 443
    models do not fit a 420px WebKit `<select>` popup, and a `<select>`
    cannot express a search field, a "Recommended" group, or a context
    window. `screens/model_picker.rs` is a bottom sheet rendered *inside*
    the settings screen — it is only ever opened from there, so it owns
    local signals instead of growing `AppCtx` for a transient concern.
    `filter_models` ranks exact > prefix > substring > display-name and
    caps rendering at 60 rows, with the "showing N of M" line making the
    cap visible rather than silently dropping the tail.

146. **Nothing is ever selected for you.** Both tier slots start unset and
    the compose screen and briefing refuse until you choose. The
    "Recommended" group is an *ordering* aid only: it resolves a curated
    id list against the live catalog, skips ids the provider no longer
    lists, and is omitted entirely when empty rather than shown as an
    empty section. Nothing in the request path consults it, and the tests
    assert exactly that (`curated_ids_never_gate_anything`). Manual
    free-text entry survives behind the sheet for a provider with no
    readable catalog — the user's complaint was that typing was the *only*
    way, not that typing should be impossible.

**Deliberately not built: shell-side "is this model in the catalog?"
validation before the request.** It looks like the obvious fix for typos,
but the catalog is cached and providers rename models constantly, so a
stale cache would reject valid ids — and a hand-entered id is
indistinguishable from a picked one in `AppSettings`. Clear error messages
(#139, #140) fix the actual complaint without adding a new
false-rejection failure mode.

**Test counts.** Workspace 192 → 209, shell 20 → 23, UI 19 → 26 (258
total). The new coverage is fixture-driven catalog parsing against a
trimmed slice of the real OpenRouter payload, the error mapper's status
branches and provider-specific hint, `filter_models` ranking, the row cap,
context formatting, the three new mock-runtime IPC tests, and a
`json_mode` request-shape test.

## Catalog bug found by testing against the live API (2026-09-27)

The picker was reported as "says 92 models loaded but I can't search or
see them". Both halves of that sentence were true, and neither was the
fault the report assumed.

147. **The tier row wiped the catalog it was about to display.** The
    `onclick` set `*catalog.write() = None` before opening the sheet,
    with a comment justifying it as "show a loading state rather than a
    stale provider's models for one frame". There was no loading state:
    the load effect only re-runs on a provider or key change, so nothing
    refetched after the click. Every open therefore produced an empty
    sheet, and the page behind it — which had the count — said "92 models
    loaded". The two statements contradicted each other on one screen,
    which is what made it read as broken rather than empty.

    The row now fetches **only if the catalog is absent** and otherwise
    opens straight into the list it already has. The sheet also renders
    `loading` and `error` itself now, because it covers the page that used
    to show them; "loading", "this failed" and "the provider has no
    models" are three states and conflating them is how a failed fetch
    reads as an empty catalog.

148. **"92 models" was the browser mock, not the provider.** OpenRouter
    serves **458** models; the parser keeps **443** (verified by
    replicating its filter against the live payload). Ninety-two is
    `invoke-shim.js`'s fixture — 12 hand-written families plus 80 padding
    rows. Under `dx serve` every shell command is replaced by canned
    data, so the list was fabricated and **no API request was made at
    all**. A mock list is indistinguishable from a real one from the
    outside, which is the same class of problem as delta #128 (a window
    rendering on mock data while looking perfectly healthy).

    The sheet's subtitle now renders `· MOCK DATA` in coral when
    `app::transport()` reports `mock`, so this cannot mislead the next
    test cycle. The real path needs `cargo tauri dev` (or a release
    build) — a plain `cargo build` binary loads `devUrl` and shows the
    browser's own error page, per the table in AGENTS.md.

149. **The live path was verified end to end with the supplied dummy
    key**, on zero-priced models only:
    * `GET /api/v1/models` → HTTP 200, 751 735 bytes, 458 models,
      `total_count` 458 — and identically 200 *without* an
      `Authorization` header, confirming the list is public while the
      key-gate remains a deliberate simplification (delta #144).
    * `POST /api/v1/chat/completions` with
      `model: "liquid/lfm-2.5-2.6b:free"` (zero-priced,
      `response_format` advertised) → **HTTP 200**, and the body matched
      what `ProviderAdapter::extract_text` reads:
      `choices[0].message.content` = `{"ok":true}`. So the base URL, the
      `Authorization: Bearer` header, the request shape and the response
      contract are all confirmed against the real service rather than
      assumed.
    * The same call against `nvidia/nemotron-3.5-lightning:free` (also
      zero-priced, `rf=False`) with `response_format` present also
      returned 200 — which corrected entry 141 above.

    Not exercised: Google's and bytez.com's model endpoints (no keys),
    and a full `master_plan` round trip through the shell (needs the
    assembled binary). The parsing half of all three is fixture-tested.

## Native-widget and cascade bugs (2026-09-27, user-reported from a screenshot)

Six defects, five of them invisible to the test suite because they live
in `wl.css` and none of them are behaviour the UI crate can assert on.
All were found by reading the stylesheet against a screenshot of the
running app rather than by reasoning about the design.

150. **The provider `<select>` was painting a near-white box with
    #E5E2E0 text on it.** Two compounding causes, and the first one is
    the one that actually mattered:
    * **The page never told the UA it was dark.** There was no
      `color-scheme` anywhere in the stylesheet, so every NATIVE widget
      rendered with the light system palette. A native `<select>` draws
      its own closed state and its own popup, and ignores
      `background-color` entirely — which is why restyling `.wl-select`
      could never have fixed it, no matter how many properties were
      added. `color-scheme: dark` is now on `:root` and
      `color-scheme: light` on `[data-theme="light"]`, so the widgets
      flip with the theme.
    * `.wl-select` also lacked `appearance: none`, which
      `.wl-horizon` on the compose screen already had. It now matches
      that control exactly — same pill geometry, same inline SVG chevron,
      plus explicit `.wl-select option` colours for the popup, which
      `appearance: none` cannot reach. Near-white text on a light native
      background is why the control read as "broken" rather than merely
      unstyled.

151. **`.wl-btn-compact` was silently defeated everywhere it was used.**
    It and `.wl-btn-ghost` have equal specificity and both set `width`,
    `height` and `font-size` — and `.wl-btn-compact` was declared ~550
    lines *earlier*, so the ghost's `width: 100%` always won and every
    compact button rendered full-width. The catalog's "Refresh models"
    is the visible instance. Two of the three pre-existing call sites
    had been rescued by one-off overrides (`.wl-inline-actions
    .wl-btn-ghost`, `.wl-drawer-close`), which is the signature of
    exactly this bug having been hit and patched locally before. The
    block is now declared *after* `.wl-btn-ghost`, so the cascade is
    honest and no new call site can hit it.

152. **`prefers-reduced-motion` was decorative.** The block was
    `* { transition: none !important; }` — transitions only. Every
    `animation` in the file still ran for users who asked for none of
    them: the backdrop fade, the nav slide, the sheet rise, the card
    enter. Both properties are now disabled, on `*`, `*::before` and
    `*::after`.

153. **The hamburger "blink" was two overlapping fades.** `@keyframes
    wl-nav-slide` animated `transform` **and** `opacity`, starting at
    `opacity: 0.6`, while `.wl-modal-backdrop` independently faded from
    `0` to `1` over 160ms underneath it. For the length of the animation
    the drawer was translucent and the canvas showed *through* it,
    superimposed on a still-lightening backdrop — a ghost of the page,
    which reads as a flicker rather than as an animation. The sheet now
    animates `transform` only; the backdrop's fade is the part that earns
    its keep, because it is the dimming.

    The nav drawer was the only sheet doing this: `.wl-drawer-sheet`
    (telemetry) has no animation at all, which is why the report named
    the hamburger specifically.

154. **Scrollbars were styled with Firefox-only properties.**
    `.wl-scroll-region` set `scrollbar-width` and `scrollbar-color`,
    which **WebKitGTK ignores entirely** — and WebKitGTK is what the
    Linux shell renders in. The result was the engine's default scrollbar,
    a bright light-grey bar down the right edge of a #131312 canvas,
    visible in the report's screenshot. `::-webkit-scrollbar`,
    `-track` and `-thumb` rules are now present, and `.wl-picker-list`
    (also a scroll container, and previously unstyled for this) inherits
    the same treatment. The Firefox properties are kept — they cost
    nothing and the browser harness may not always be WebKit.

155. **The model picker shared a z-index with the toast.** Both were 60,
    and the sheet is bottom-anchored while `.wl-toast` sits at
    `bottom: 86px` — squarely on top of the model list. The picker's
    backdrop is now 80. The other overlays are deliberately left BELOW
    the toast at 40: the telemetry drawer flashes a toast when you sync
    from inside it, and raising its backdrop would hide the result of
    the button you just pressed.

156. **The embedded release bundle was 8 hours stale.** `frontendDist`
    still held the 12:57 build — predating the model picker entirely, so
    a `cargo tauri build` binary would have shipped a UI without any of
    it. `scripts/build-ui.sh` has been re-run (wasm 942 810 B, 90% of
    the 1 MiB budget, prune + size check passed) and the minified
    output was inspected to confirm the fixes above survive
    minification *and* keep their cascade order — `.wl-btn-compact` is
    emitted after `.wl-btn-ghost`, which was the whole point of 151.

**Not verified interactively.** No desktop browser was attached, so
none of this was observed rendering. The fixes are verified by reading
the source and by confirming them in the built artefact; 153 in
particular is a diagnosis of the animation from the CSS, not a
reproduction. If the hamburger still blinks, the next suspects are the
backdrop fade itself (`160ms` on `--ease-tactile`, which front-loads
almost all its motion into the first ~30ms) and the canvas's
`wl-card-enter` re-running behind the scrim.

## Dead shell surface (2026-09-27)

157. **`create_manual_milestone` / `create_manual_directive` are
    deleted.** Delta 74 shipped both as the authoring surface for
    "Phase-2 MVP-1", and delta 118 counted them among the four
    commands with multi-word parameters that needed
    `rename_all = "snake_case"`. Nothing ever called them: the compose
    screen's manual path sends the whole plan to the single
    `create_goal`, which seeds the starter milestone + directive
    itself (B-002). The commands were reachable from `generate_handler!`
    and mocked in `invoke-shim.js`, so they read as a live contract —
    but the only thing exercising them was `ipc_tests.rs`, i.e. the
    test was the sole proof of a feature no user could reach. The
    same dead-code call as delta 119 (`Screen::ByokSetup`), from the
    other direction: that one was unreachable *in the UI*, these two
    were uncalled *from* it.

    Removed: both `#[tauri::command]` fns, their two
    `generate_handler!` entries (`src/main.rs`), their two
    `invoke-shim.js` fixtures, and the `create_manual_*` half of the
    manual-path IPC test. `Repos::create_milestone` /
    `Repos::create_directive` are **untouched** — `AiDispatcher::persist_plan`,
    `persist_briefing`, and most of the core/engine/store/sync test
    suites call them directly, so no core surface was orphaned.

    The IPC test keeps the coverage that actually mattered. The two
    commands' distinctive risk was a *silently* defaulted optional or
    integer field — the delta-118 failure mode, where the call
    succeeds and the data is simply gone. That risk survives on the
    path users take: `create_goal`'s `target_date` is asserted in the
    response **and** read back off the persisted goal row, and the
    seeded directive's `execution_context` (an `Option`, sourced from
    the goal description) and `estimated_minutes` (never sent by the
    UI) are asserted off the row. `master_plan` covers the remaining
    multi-word key set.

## The control panel (2026-09-27, user-directed)

158. **The hamburger drawer is a control panel, not a menu.** It was two
    buttons ("Create a goal", a gear) pinned to the bottom of a 273px
    sheet with a `flex: 1` spacer above them — the void *was* the layout.
    The user's framing: "It should not be a list of folders; it should be
    a map of the user's operational reality." It now carries a primary
    action, a **System Telemetry** group, an **Active Worldlines**
    readout, and Settings, with a fixed top bar and a fixed Settings foot
    around a scrolling middle — `flex-shrink: 0` on both is what keeps
    Settings reachable once a user has six goals.

159. **Two new pages, both read-only.**
    * **Entropy Log** — the escape-hatch ledger, which until now was
      write-only: the reason was recorded, counted into velocity, and
      never shown again. Grouped by goal (a bailout only means something
      next to what it derailed), with a pattern line
      (`4 events · 2 energy · 1 scope · 1 external · 1 still blocked`).
    * **Trajectory** — required velocity against observed, which existed
      but was only reachable *after* the fact as the evening audit's
      receipt. Opening it before the check-in is the moment the number
      can still change a decision.

    Read-only by explicit decision. A blocked directive has no unblock
    path in the engine (delta 33), so a tap target here would be a
    control that cannot do what it says.

160. **The canvas lost its Complete/Bailout footer.** The user's call,
    taken knowingly over a recommendation to keep the cream CTA: the
    canvas is the directive, not a dashboard, so completion is `⌘+Enter`
    and the escape hatch is `Escape`, and the menu is the only control on
    the surface. The recorded cost is discoverability — nothing on screen
    now names either shortcut. The escape modal is untouched; only its
    trigger moved. `.wl-actions` is deleted; `.wl-btn-primary` and
    `.wl-btn-escape` survive everywhere else.

161. **Two new shell commands, both no-argument.** `list_goals` reuses the
    existing `GoalJson` (no parallel struct) over a new
    `Repos::active_goals_with_progress` — one LEFT JOIN + GROUP BY, so a
    goal with zero milestones still appears. `entropy_log` returns a new
    `EntropyView` over `Repos::bailout_log`, a four-table join.
    `bailouts` has **no date column**; the date is derived from the HLC's
    physical component, which is nanoseconds since the epoch.

162. **Blocked entries are badged, not listed twice.** `bail_out` parks an
    `external_dependency` directive in `blocked` forever, while scope
    downsizes-and-requeues and energy skips — both resolving on the spot.
    So the ledger is the spine and `still_blocked` flags the residue.
    Two lists would have double-counted the blocked ones.

163. **An unrecognised bailout reason is counted, not dropped.** The
    summary's breakdown is `total` minus the three known reasons, so a
    future shell adding a fourth cannot make the page under-report its own
    headline. This one was caught by the test written for it.

164. **The worldlines are not buttons.** No hover state, no pointer
    cursor. They are capped at four with a `+N more` line, because a fifth
    row pushes Settings off the bottom of a 747px panel, and Settings is
    not optional. Making them tappable would promise a goal-detail page
    that does not exist.

165. **The panel marks its own mock data.** `list_goals` under `dx serve`
    is invented, not queried, so the System Telemetry label renders `· MOCK`
    in coral when `transport()` reports the mock — the same guard as delta
    148, for the same reason: a fabricated list of the user's own goals is
    indistinguishable from a real one.

166. **Two icons, both refusing the obvious mark.** Entropy is *not* a
    warning triangle — this system has no red failure state, and a hazard
    sign would be the first one. It is a trace that rises, breaks, and
    resumes lower; the gap is the idea. Velocity is a gauge, not a rising
    line, because the two sit 12px apart in the same drawer and a line
    would read as the same mark twice. The needle points at roughly two
    o'clock: a gauge resting in its low corner would itself read as
    failure.

## Tier-2 and the morning briefing are gone (2026-09-27, user-directed)

167. **The morning briefing and the Tier-2 Tactical Dispatcher are
    deleted, whole.** The product owner's call, against my recommendation
    to keep the engine: "i dont want a morning briefing page, the main
    page should be 'No active directive. Open the menu to create a goal
    or review settings.' with the hamburger menu." The canvas was already
    exactly that; the brief was the thing being removed.

    The brief was the **only** surface for Tier-2 — the 1–3 directives a
    day, authored from the active milestone and 48h of velocity. With
    delta 160's canvas reduced to the directive alone, there was nothing
    left for a second AI tier to justify itself against, so the whole
    path went rather than being left dormant. Same call as delta 119
    (`Screen::ByokSetup`) and delta 157 (`create_manual_*`).

    Removed: `screens/morning_brief.rs`, `Screen::MorningBrief` + its
    dispatch arm, the `BriefingView` DTO, the module entry and re-export,
    the shim mock, `.wl-brief-greeting`, the `morning_briefing` command
    + `generate_handler!` entry, its half of the blank-model IPC test, and
    in `wl-core`: `BriefingResult`, `parse_briefing`, `validate_briefing`,
    `persist_briefing`, `AiDispatcher::morning_briefing`, the
    `tier2_system`/`tier2_user` prompt builders, `MAX_AI_CONSTRAINTS_CHARS`,
    the `BriefingResult` re-export, and three tests. Workspace 218 → 216.

168. **Tier-1 kept its whole surface.** The Master Architect is still
    reachable from the compose screen and still names the goal and builds
    the milestone tree — the AI earns its place by producing the plan you
    execute, not by producing a daily list on top of it. `parse_plan`,
    `persist_plan`, `tier1_*` and their tests are untouched.

169. **`AppSettings.tier2_model` stays, and is now inert.** It is a column
    on the CRDT-synced `app_settings` row; removing it needs a schema
    migration, and the settings UI still surfaces a "dispatcher model"
    picker that no longer drives anything. This is the one knowingly
    dead surface left by the cut, recorded here rather than hidden: if a
    Tier-2 returns it must be rebuilt, and this column is where it
    resumes. Everything else went.

170. **The Dormant screen lost its second button.** It led to the brief
    and to the canvas; with the brief gone it is a rest beat with one
    affordance, "Return to the line", which is the honest shape for a
    screen whose whole job is to say stop.

## The 12px floor, and a system that stopped drifting (2026-09-27, user-directed)

A whole-app redesign from two pieces of user feedback: the hamburger panel
did not look consistent, and the section labels ("INTELLIGENCE", "AI
PROVIDER", the Nemotron model name) were unreadable on a small screen. The
pin-above-other-windows feature was removed at the user's request.

171. **12px is now a hard type floor, and every size is a named token.**
    The stylesheet had 28 `font-size` declarations in the 9–11px band
    across eleven classes, and the skill's own floor for mono telemetry was
    12px — so the CSS had drifted below its own specification. Settings
    stacked three label tiers at 11px / 11px / 12px that differed from one
    another by one pixel of letter-spacing, which is what made
    "INTELLIGENCE" and "AI PROVIDER (BYOK)" read as one undifferentiated
    smudge rather than as a heading and a field. Nine tokens now
    (`--t-display` … `--t-mono`), and the only bare pixel sizes left in the
    file are 18 / 22 / 24 / 26 / 30px, each with a comment saying why it
    is not on the scale.

172. **Section headings are 17px sans in sentence case, not uppercase mono
    micro-caps.** This reverses §2 of the skill, which previously called
    for 10–11px mono caps. Uppercase small mono was doing a real job —
    making a label distinguishable from the content beneath it — by a means
    that does not survive a 420px window. Group identity now comes from
    size and weight relative to what is labelled, which survives any
    viewport. The mono eyebrow survives at 12px for genuine telemetry only:
    HLC stamps, counts, `MOCK DATA`, `SEALED`.

173. **`--wl-text-tertiary` is gone, and it was the cause of the
    "inconsistent hamburger".** It was referenced by thirteen rules and
    defined in none of them. A `color:` declaration naming an undefined
    custom property is invalid at computed-value time, so every one of them
    silently INHERITED — and in the control panel that meant "SYSTEM
    TELEMETRY" and "ACTIVE WORLDLINES" rendered at full
    `--wl-text-primary` linen, brighter than the 15px rows they were
    heading. It also inverted `.wl-nav-chevron`, the worldline counts, and
    every label on the Entropy Log and Trajectory pages. The text ladder is
    now three steps (primary / secondary / muted), all clearing 5:1 on the
    card surface, and the rule is absolute: **a group label is quieter than
    what it labels.**

174. **`.wl-chip` had two definitions and the second silently won.** One
    10px uppercase pill with an elevated background, one 9px inline
    lozenge with a transparent one. The "sealed" key chip in Settings and
    the "blocked" chip in the Entropy Log were therefore the same class at
    two different sizes. Collapsed to one 12px definition. `.wl-hud` is
    gone entirely along with its last user.

175. **Pin-above-other-windows is removed, everywhere, with no migration.**
    The switch left Settings, the "Pin to top:" line left the telemetry
    drawer, `set_always_on_top` and its `generate_handler!` entry left the
    shell, the boot-time restore left `main.rs`, and `AppSettings.
    always_on_top` left `wl-core`, the UI DTO, and the shim mock. The
    `app_settings.always_on_top` COLUMN stays, on the same terms as the
    `hotkey` column before it: dropping a column on a CRDT-synced singleton
    costs a migration and buys nothing, because it keeps its
    `NOT NULL DEFAULT` and SQLite supplies it for the omitted INSERT field.

    The consequence worth stating is what happens to a device that has not
    been updated: it keeps pinning its own window and keeps pushing the key,
    and the new side ignores it. Two tests hold that line —
    `settings_survive_a_row_written_by_a_build_that_still_had_the_pin`
    (wl-core) and `a_settings_op_from_an_old_peer_still_applies` (wl-sync).
    The second is the one that matters: REJECTING an old payload would wedge
    that device's sync permanently, whereas ignoring a key it has stopped
    caring about degrades cleanly.

176. **The native `<select>` is retired from Settings.** The provider
    chooser shipped as a **near-white field carrying #E5E2E0 text** inside
    a graphite card. It was "fixed" once, with `color-scheme: dark` in
    `:root` plus `appearance: none`, and restyled once, and it failed
    again: `color-scheme` darkens the dropdown *popup* and `appearance: none`
    styles the *closed* state, and neither reliably reaches the widget in
    WebKitGTK. Restyling it a third time would have been the wrong move, so
    the provider became a `.wl-value-row` opening a bottom sheet, the same
    control as the two model slots. Settings now contains **no native form
    widget at all** — one row type, one open gesture, one list rendering.
    The compose screen's target-date horizon is the app's last `<select>`
    and earns the exemption: a small closed control with no catalog to
    search and no history of painting itself white.

177. **`.wl-value-row` is the component for "a value chosen from a list",
    and the value is 13px mono, not 12px.** A model id is the value that
    gets sent and is compared character by character against a catalog; at
    the telemetry size with `word-break: break-all`,
    `nvidia/nemotron-3.5-lightning:free` was one squeezed line. It is
    `--t-mono-lead` with a two-line clamp now. The row's third line is a
    fact (the model's context window) or it is absent — no placeholder, no
    `—`, because a rendered placeholder trains the eye to skip the line.

178. **One circular control, defined once.** `.wl-float-btn` (the canvas
    menu) and `.wl-back` (every secondary page's chevron) were separate 34px
    definitions at different offsets, so the two controls occupying the
    same corner on the same screen did not line up — which is most of what
    "the hamburger looks inconsistent" meant in practice. Both now derive
    from a single 38px `.wl-circle-btn`. The canvas button also carried a
    `wl-float-menu` class with no rule anywhere in the stylesheet, so its
    specificity was a mirage; that modifier now exists and does the one
    thing it should.

179. **The last three font glyphs are now inline SVG.** `✚` on the panel's
    primary action, `›` on every trailing row, and `✕` in the choice sheet
    were each defended as "unambiguous at this size". That was true of
    their shape and false of their rendering: the same three marks sat at
    three different sizes and baselines across three sheets because the
    font stack decided, which is exactly the drift the SVG set exists to
    remove. `IconNew`, `IconChevron` and `IconClose` replace them. The
    evening check-in's `●`/`◐`/`○` is the sole remaining glyph, and it
    stays because a half-filled circle is a typographic idiom with no
    universally recognised drawn equivalent.

180. **Settings has one save path: every field commits on `change`.** The
    page previously had two — appearance switches applied and persisted on
    click, text fields staged into a draft for a "Save settings" button —
    and said so on screen, in a note under the button, which was an
    admission that the model was wrong. Both are gone. `onchange` fires on
    blur or Enter, never per keystroke, so an abandoned half-typed relay URL
    is never written.

    `settings_save` overwrites the whole settings row, so a commit cannot be
    a patch. The rule is now: **send the last shell-confirmed state with
    only the field the user just changed applied.** This is the old
    `switch_payload`, generalised — the theme switch had that bespoke
    payload for exactly this reason and was the only field that had it.
    Flipping the theme can no longer save a half-typed relay URL; picking a
    model can no longer save a half-typed API key. The one field
    deliberately wider than its name is the provider, which also commits
    both cleared model ids, because a stale id is a 404 the user cannot
    interpret. A failed commit now REVERTS the draft to the last confirmed
    state and says `NOT SAVED — …`; the old code flashed an error and left
    the unsaved value on screen.

181. **The control panel is 312px and hugs its content.** It was 273px,
    which left ~200px of label column after the icon gutter — too narrow for
    a goal title and a count on one line. The sheet is now a card anchored
    top-left with `max-height: 100%`, so the long case still scrolls inside
    the frame. Three separate designs have now tried to balance this panel
    and all three produced a void: buttons pinned to the floor behind a
    `flex: 1` spacer, then centred groups to "fill" a stretched sheet, then
    full-height with a footer. The rule now encoded is that content ends
    where it ends, and short is the normal case because most installs have
    one or two goals. `WORLDLINE_CAP` dropped 4 → 3 for the same reason.

182. **`.wl-section` fields are separated by a 1px hairline, not by 14px of
    air.** This turned Settings from a stack of disconnected boxes into
    cards that read as cards, and reclaimed roughly 40px of a 747px window.
    It is the single highest-value layout change in the redesign and it
    cost one line of CSS.

183. **A static `preview.html` harness was added, and then removed (2026-09-27,
    user directive: no standalone HTML in this project).** While it existed it
    rendered every screen's markup against the real `public/wl.css` with no
    wasm and no Tauri shell; it was added because the only available
    verification path was a headless screenshot of a partially-booted wasm app
    — which caught nothing — and the defects above were found by reading a
    stylesheet. It is gone, along with every reference to it. The verification
    path is `scripts/dx.sh serve --port 1420`, which draws the real screens
    against the same stylesheet in a browser behind the mock shell. The
    original diagnosis stands: two of the defects above (the cascade-ordering
    bug in §11 of `wl.css`, and the dead `--wl-text-tertiary`) were only ever
    visible rendered.

## Two defects found in the shipped build (2026-09-27, user-reported)

184. **A coral focus ring was drawn around the entire 420×747 frame on
    every launch.** The focus-ring rule ended in a bare
    `[tabindex]:focus-visible`, which has specificity (0,2,0) and therefore
    beat `.wl-root`'s own `outline: none` at (0,1,0). The canvas root
    carries `tabindex="0"` and `autofocus` precisely so it can receive the
    ⌘+Enter and Escape keydowns — which means it is focused
    *programmatically on every boot*, which means the ring was not an edge
    case but the first thing on screen.

    The fix is `:not(.wl-root)` on the ring rule rather than a stronger
    `outline: none` on the root, and the distinction matters: those
    `tabindex` roots are EVENT TARGETS, not controls. A focus ring exists
    to tell a keyboard user where they are among things they can operate;
    a wrapper focused so it can observe a keystroke is not one of them.
    Deleting `[tabindex]` from the rule outright would have been shorter and
    would have silently un-ringed any future genuinely-focusable custom
    control, so the exclusion is explicit.

   Note what the fix is NOT: the `tabindex` is still there and the element
   is still focusable. Suppressing the ring does not suppress focus, so
   ⌘+Enter and Escape are unaffected. The regression is guarded in
   `crates/wl-app/ui/src/app.rs` instead: the canvas root's `tabindex="0"`
   and its `autofocus` are asserted there, so the ring cannot later be
   "fixed" by deleting the focus target.

185. **The control panel is full height again, at the user's direction.**
    Delta 181 had the sheet hug its content. The user rejected that: a
    drawer that ends mid-air leaves the canvas visible beneath it, and
    there is no way to tell from the outside whether the tap missed or the
    surface is broken. The sheet is now `height: 100%`, top edge to bottom
    edge.

    That reverses the *fix* in delta 181, not its *diagnosis*. The void was
    real — a 747px sheet whose content ends at 400px has 350px of nothing
    in it. The mistake was treating "the panel is too tall" as the problem
    when the problem was "the panel has nothing to do with the extra
    height". So the slack now goes to the worldline readout
    (`.wl-nav-worldlines { flex: 1 }`), which puts the empty space INSIDE
    the list region where a list is expected to have room, instead of below
    the whole panel. The fixed things — the primary action, the two System
    rows — keep their rhythm at the top, and Settings stays pinned to the
    floor where a thumb expects it.

    `justify-content: space-between` on the scroll region was the other
    candidate and is wrong: it spreads the gaps evenly and pulls the
    primary action away from the top of the sheet, which reads as a broken
    layout rather than a tall one.


## Battle test (2026-09-27, user-directed: find every bug, near-zero CPU/RAM, strong security)

A seven-way parallel audit of every crate plus a targeted adversarial
campaign against the hostile boundaries. Twenty-two defects fixed; every
one is now locked by a named regression test.

### Remote, unauthenticated, and permanent

186. **One pulled op could brick a client forever.** `Hlc::increment` carried
    a counter overflow into `physical.checked_add(1).expect("HLC timestamp
    exhausted")`. `Hlc::observe` adopted a remote timestamp's physical
    component *verbatim* — the wire HLC is a cleartext routing header — so
    one op carrying `18446744073709551615.65535.00001` drove the head to the
    ceiling, and the next local write panicked in the write path. The head is
    persisted to `hlc_clock`, so it survived every restart, and there was no
    recovery short of deleting the data dir. Two `#[should_panic]` tests
    pinned the panic as intended behaviour; both are deleted.

    `observe` now bounds the remote physical to `MAX_REMOTE_DRIFT_NANOS`
    (one hour) above the local wall clock — which is what this module always
    promised ("bounded-drift from physical time") and had never enforced —
    and `increment` saturates instead of panicking. Saturating is the right
    degradation: `(physical, counter, device, operation_id)` is a *total*
    order even with repeated `(physical, counter)`, so duplicated ticks cost
    strict monotonicity while a panic costs everything.
    (`wl-core/src/hlc.rs`; `defect_maxed_wire_hlc_cannot_brick_the_local_clock`.)

187. **The relay could forge any op's merge order.** The AEAD's associated
    data was `table:record` alone. `hlc` and `operation_id` are cleartext
    `PushOp` fields, so a relay could re-stamp an op to the top of the key
    space to make it win every merge, relabel it to defeat the
    `(device, operation_id)` tie-break, or rewind it so a real user's edit
    lost — and none of that was detectable, because none of it was in the
    associated data. Zero-knowledge held for the *payload* and not for the
    *ordering*, which is what actually decides whose data survives.

    The AAD is now `wl/v2:{table}:{record_id}:{operation_id}:{hlc}`, built by
    one function (`crdt::routing_aad`) that both sides call. The relay keeps
    everything it needs — it still reads the cleartext columns for
    `ORDER BY hlc` and the pull cursor, and still never sees a plaintext
    byte. This is a wire-format break, deliberately, rather than a shim: a
    `v1` row cannot be authenticated under `v2` and is quarantined on the
    next pull, which is the correct fate for a pre-release format rather
    than silently accepting metadata the tag does not cover.
    (`defect_relay_rewriting_the_hlc_is_detected_and_quarantined`,
    `defect_relay_relabelling_the_operation_id_is_detected`.)

188. **One crafted op wedged sync permanently.** `is_fatal_store_error`
    allow-listed CHECK/FK/PK/UNIQUE but not NOT NULL, and SQLite checks NOT
    NULL *before* CHECK. A single sealed `{}` bound `NULL` into
    `goals.title`, was classified fatal, and returned from the cycle *before*
    `save_cursor` — so the client re-pulled and re-failed identically on
    every future sync, forever. Unrecoverable without deleting the database.
    (`defect_not_null_violation_quarantines_instead_of_wedging_sync`.)

189. **A poison check-in destroyed a real one.** `apply_check_in_win` deleted
    (and tombstoned) the same-date rows it was displacing *before* inserting
    the winner, with every statement autocommitting. A payload with a
    missing or out-of-range `outcome` failed the INSERT after the DELETE had
    already landed, and the quarantine path then watermarked the op as seen —
    so the user's own check-in could never come back on that device while
    its peer still had it. The whole apply is now one transaction.
    (`defect_poison_check_in_cannot_destroy_a_real_one`.)

190. **A one-step delete erased every phase on every peer.** All steps of a
    progressive directive shared the directive's record id, so they were one
    merge record: the peer's apply ran `DELETE FROM directive_phases WHERE
    directive_id = ?`. Deleting step 2 on one device wiped step 1 everywhere.
    The coarseness also meant an op for step 1 lost to an unrelated step-2 op
    that merely carried a newer HLC. Phase records are now
    `"{directive_id}:{step}"`, and the step is read from the KEY rather than
    the payload, so the key the merge head arbitrates on is the key the write
    lands on. (`defect_deleting_one_phase_keeps_the_others_on_peers`.)

191. **Replicas diverged for good on an exact HLC tie.** Resurrection was
    decided by `ts > tombstone_ts`, ignoring the `(device, operation_id)`
    tie-break that had just decided the op was accepted. Two replicas that
    received a tombstone and an upsert sharing a full HLC in opposite orders
    ended in different states — permanently, and specifically in the
    clone-split scenario the tie-break exists for. The key is the only
    arbitration there is, so an op that beats the head now resurrects
    outright. Stale upserts still lose and still cannot resurrect, so B-003
    is unchanged. (`crdt::TableState::apply` and its SQL mirror, together —
    the in-memory one is the reference contract.)

192. **A stale remote op could clobber a newer local edit.** Only the delete
    writers maintained `record_heads`, so the head lagged behind the row on
    every ordinary write — and the pull side prefers the head over the row's
    own HLC. Peer op at T_r lands (head = T_r) → the user changes a setting
    at T_l > T_r (row = T_l, head still T_r) → a third device's op at T_m
    between them beats the head and blind-upserts the stale value over the
    user's edit. Every local upsert now advances the head, which is what the
    head is for. (`defect_stale_remote_op_cannot_clobber_a_newer_local_write`.)

### Availability and cost, on the hostile side of the relay

193. **The documented production build panicked on startup.**
    `PostgresStore::open` is synchronous and drives sqlx through
    `Runtime::block_on`, which tokio refuses to call from a thread already
    inside a runtime. Under `#[tokio::main]` that thread is one, so
    `--no-default-features --features postgres` — the configuration the
    `compile_error!` points operators at — died before binding a socket. The
    store is now built before any runtime exists.

194. **The pull byte budget under-counted by up to 5×.** The estimate summed
    raw `str::len()` of the routing headers, but serde_json escapes a
    control character as six bytes, so a 128-character header of U+0001
    counted as 128 and serialised as 768. The response sailed past
    `MAX_PULL_BYTES`; the client reads with exactly that ceiling, so it
    rejected the page; and because the cursor is only saved from a *decoded*
    response it never advanced. The account could never sync again. The
    budget now measures the op as serialized.
    (`defect_pull_byte_budget_counts_the_serialized_op`.)

195. **A failed signature did not burn the challenge.** The claim ran only
    after `verify_strict` had already succeeded, so a bad signature left the
    nonce in the map and the four nonces `MAX_CHALLENGES_PER_ACCOUNT` allows
    served unbounded retries — each paying point decompression plus a
    double-scalar multiply, the deliberately slow variant, on an
    *unauthenticated* route. One keypair could hold the relay's CPU
    indefinitely. The challenge is now consumed first, before the expensive
    part. A real client loses nothing: the nonce is a secret it holds, and a
    fat-fingered signature just means asking for another.
    (`defect_bad_signature_burns_the_challenge`.)

196. **Every challenge request did two O(n) sweeps under the mutex.**
    `issue_challenge` retained the whole challenge table and then swept the
    whole session table, on an unauthenticated route, while holding the
    challenge lock. Filling the table once turned every later challenge
    request into ~1 ms of convoyed work. The sweep is now throttled to every
    64th request, the session sweep is gone from that path entirely
    (`validate` already runs it on a throttle), and the global ceiling is
    sized for the hot path rather than for headroom. Between sweeps an
    expired entry keeps its slot, which makes the caps read slightly
    conservative — the right direction to be wrong in.

197. **The account cap ratcheted shut forever.** Registration is
    unauthenticated and the public key is caller-chosen, so 10 001 requests
    with 10 001 random keys filled `accounts` permanently: the old code
    deleted only the row it had just inserted, never reclaimed the previous
    10 000, and the table is a file that outlives the process. Every
    legitimate new device then got 429 on `/auth/challenge` and could never
    onboard, with no remedy but a manual database edit. The cap is now a
    sliding window — over the cap, the oldest accounts holding no ops are
    evicted, down to a 90 % low-water mark so the sweep runs once per batch
    of registrations rather than once per registration. An evicted account
    re-registers on its next challenge; one holding ops is never evicted.
    (`defect_unbounded_unauthenticated_account_registration`, rewritten —
    the old version asserted the lockout as correct.)

198. **No request deadline and no concurrency ceiling.** A client that opened
    a connection and dribbled a body held a task and its read buffer open
    indefinitely, and the in-flight count was whatever the attacker chose —
    unbounded RAM growth on an unauthenticated socket. The router now carries
    a 30 s deadline and a 256-request ceiling. Both bounds sit above any
    honest deployment and below what a connection flood can open.

### Correctness in the engine and the store

199. **A progressive directive could become uncompletable.** `complete()`
    bypassed the phase self-heal that `activate_next` runs, and
    `advance_progressive_step` hard-fails on a missing phase row — so once a
    truncated pull lost the rows, every ⌘+Enter errored until the canvas
    happened to reload. The repair now runs first, and the advance's return
    value is honoured instead of discarded: it is `false` when the row moved
    under a concurrent merge, and the old `?` reported the directive as still
    active at `phase: (total, total)` having advanced nothing.

200. **A milestone was completed with unreachable work inside it.** The
    outstanding set was `('queued','active','blocked')`; `skipped` was left
    out, so a milestone with one energy-bailed-out directive and one
    completed directive was marked `completed` while the skip sat unstarted
    — and nothing requeues a skip, and `next_runnable_directive` only selects
    `queued`. It was then reported as done while real work sat inside it.

201. **A bailout could be recorded for something that never happened.** The
    ledger row committed before the state transition, so a failure in the
    second step left a bailout replicated to every device describing an event
    that did not occur, while the directive stayed active. The ledger is
    written last.

202. **A replicated `progressive_step` past the total wedged a directive
    forever.** The two columns have no cross-field constraint and the pull
    path writes both from the payload. The step has no phase row, so every
    engine path that read the current phase errored, and nothing could
    requeue it. The step is now clamped into `1..=total`.

203. **The phase repair manufactured the divergence it existed to heal.**
    `ensure_phases` updated only `minutes` locally but emitted a SYNTHESIZED
    row (`title = "Step N"`, `instruction = None`, `state = active|pending`)
    under a fresh, winning HLC — and it recomputed equal slices, so a
    directive authored as 5 + 25 came back as 15 + 15. A peer lost the real
    title, the instruction and any `done` state. It ran on every canvas load,
    so one truncated pull was enough to trigger it. A step that already
    exists now keeps everything and emits nothing.
    (`defect_phase_repair_does_not_erase_titles_or_state`.)

204. **`ensure_phases` sized a `Vec` from an unbounded replicated integer.**
    `progressive_total` has no schema constraint and the only guard was
    `estimated_minutes >= count`; one op carrying `estimated_minutes = 10^18,
    progressive_total = 10^9` reached `Vec::with_capacity(1_000_000_000)` on
    the path the engine hits on every canvas load.

205. **⌘+Enter and Esc on an idle canvas started a directive.** Both fell
    through to `activate_next` when nothing was active. Activating is the
    canvas load's job (`current`), and it is what does it now.

206. **`activate_next` wrote before it validated.** `set_directive_state(..,
    Active)` committed and then `ensure_milestone_active` could fail, leaving
    an active directive whose milestone was never promoted, with the caller
    holding an `Err` and no idea the activation had landed.

207. **A milestone already `completed` was left behind a live directive.**
    Only `pending` was promoted to `active`, so a directive created against a
    finished milestone (by a peer, or by re-planning) ran with the milestone
    still marked done.

208. **The metadata-hostname SSRF block was bypassable with one character.**
    `metadata.google.internal.` is the same name to the resolver — the URL
    spec's domain-to-ASCII does not strip the root dot — but it is neither
    equal to nor `.ends_with()` the blocked form. The root label is now
    trimmed before comparison. The IPv6 arm also unwrapped only
    IPv4-*mapped* addresses, so the deprecated `::a9fe:a9fe` form of
    169.254.169.254 sailed through while `connect(2)` still honours it;
    `to_ipv4()` handles both. `relay_url` is a replicated field a hostile
    peer can write, so this was reachable.
    (`defect_metadata_hostname_cannot_be_reached_with_a_trailing_dot`.)

209. **The pull cursor could move backwards.** `save_cursor` was an
    unconditional overwrite, and Settings and the telemetry drawer each own
    an independent sync button — so two cycles can run concurrently against
    one `Repos`, and the slower one overwrote the faster one's cursor on the
    way out. Re-pulling that window is merely wasteful, except the
    watermarks had already been pruned, so every op in it was re-decrypted
    and re-applied on a fully-synced device. The write is now monotonic, and
    it validates the cursor rather than persisting anything the relay would
    reject.

210. **The Stackelberg repair failed silently and was never retried.** The
    result of `enforce_single_active` was discarded, and the call was gated
    on `applied > 0`, so a cycle that pulled nothing new — the common case
    while catching up on a failed repair — never re-ran it. Two `active`
    rows could persist while the shell reported SYNCED. It now runs every
    cycle and propagates.

### Smaller, but real

211. `wall as i64` persisted an HLC physical component by *reinterpretation*,
    not conversion, so a clock past `i64::MAX` stored a negative timestamp in
    the column every comparison reads. Saturating now.
212. `identity()` mapped a corrupt `verify_indices` to "no challenge" via
    `unwrap_or_default`, so the shell silently re-prompted onboarding and the
    bad row stayed invisible. Every other mapper surfaces its parse failure.
213. `verify_signature` used the lax `verify`, which skips small-order and
    non-canonical-key rejection. `verify_strict` costs the same.
214. `lock_conn` swallowed a failed ROLLBACK, which breaks the exact
    guarantee the function advertises. Now logged.
215. Session tokens were `"{counter}-{uuid}"`, handing every token holder a
    per-process session counter and an instance fingerprint. Purely random
    now.
216. A hex-valid, 64-character public key that is not a curve point was
    answered 500 — unauthenticated 5xx generation, with no log line, because
    the only `tracing::warn!` in the file was on the challenge path.
217. `MAX_CHALLENGES_GLOBAL` (100 000) and `MAX_SESSIONS_GLOBAL` (100 000)
    were unreachable given `ACCOUNT_CAP` = 10 000. The challenge ceiling is
    now sized for the hot path and reachable.

### Tests that were pinning the defects

Eight tests asserted behaviour that was itself the bug, and were rewritten
rather than preserved: the two HLC `#[should_panic]` tests, the phase
tombstone keyed by directive id, the relay account-cap lockout, the
`save_cursor` "API exists" probe, the completion loop that relied on ⌘+Enter
auto-activating, the vacuous `missing_phase_rows_are_rebuilt_from_the_estimate`
(it scheduled its fixture for 2099, so `activate_next` picked a different
directive and never exercised the repair it names), and the same-day check-in
convergence test that asserted `(applied, quarantined) == (1, 0)` for an op
that had stopped quarantining.

### Verified by battle test, not by assertion

218. **The live chain was exercised end to end over real TCP, not only in
    process.** A throwaway harness drove a release `wl-relay` over a real
    socket: `/auth/challenge` -> Ed25519 `/auth/verify` -> sealed push ->
    pull -> decrypt -> LWW apply on a second device, which converged on
    the written goal. It also confirmed the two properties that are easy
    to claim and hard to prove: **replaying a consumed challenge returns
    401** (the challenge is strictly one-shot, and now burns on a bad
    signature too), and **an anonymous `/sync/push` returns 401**. The
    harness is deleted; the assertions it made permanent live in
    `wl-relay/tests/adversarial.rs` and `wl-sync/tests/adversarial.rs`.

    Idle cost, measured on the release binary rather than asserted: **0
    ticks in 30 s (0.0000% of one core), 6 MiB RSS, 13 threads** for the
    relay. The debug binary costs 0.067% and 85 MiB, so the number that
    matters for a shipped install is the smaller one.

219. **`prune-dx-dist.sh` silently did nothing on an absolute path.** It
    unconditionally prefixed the repo root, so an absolute dist
    directory — which its own usage line invites — resolved to a
    nonexistent path and exited 0 having pruned nothing. A clean exit
    from a pruner that pruned nothing reads as a pass.

220. **The release bundle shipped two debug HTML files that were not in
    `public/` any more.** `dx build` copies `public/` verbatim and never
    deletes, and the pruner only ever looked at `assets/*-dx*`, so
    anything that had ever lived in `public/` stayed in the dist forever
    and was embedded into the shipped app by Tauri. Removing the source
    was never sufficient; the artefact has to be swept too. The pruner now
    sweeps the bundle root against an allowlist plus whatever `index.html`
    actually references.

221. **A `tauri.conf.json` comment key is not free.** Documenting the
    `csp` / `devCsp` split with a `_why` key fails the build:
    `tauri-build` validates the security schema and rejects unknown
    fields. The rationale therefore lives in `build.rs`, next to the rest
    of this failure mode. Worth recording because the obvious way to
    annotate JSON is unavailable in exactly the file where a reader most
    needs it.

### Removed

222. **Temporary diagnostic scaffolding, both halves.** `__temp::diag_sink`
    was registered in `generate_handler!` — a live, webview-reachable IPC
    command appending an attacker-controlled string to a fixed path with
    no size, encoding, or path bound. Its `PROBE` script was handed to
    `win.eval()`, the one mechanism in the app not subject to the CSP,
    and installed capture-phase listeners over the whole document
    (**including the seed-phrase and API-key fields**) plus a
    `setInterval(…, 300)` that only cleared on the happy path. A parallel
    copy in the UI crate called `settings_save` on every probe tick. All
    of it is gone, and `grep` for `__temp` / `diag_sink` now returns
    nothing. The mount-point guard (`__wl_mounted__`) is unrelated and
    stays.

223. **A dev binary that looks broken when it is not.** `build.rs` already
    warned at compile time, but the person who needs the message is the one
    *running* the binary. `main()` now prints it to stderr on launch
    whenever `custom-protocol` is off, naming both correct commands
    (`cargo tauri dev`, which starts the server; and a `custom-protocol`
    build, which embeds the bundle). Diagnostics only — no blocking, no
    fallback, no behaviour change.

## The control panel had no owner, and the canvas's control had no surface (2026-09-27, user-reported: "the hamburger menu icon doesnt works")

**Reported as:** tapping the canvas's hamburger does nothing.

**What was measured, before anything was changed.** The failure was
reproduced-or-not inside the real WebKitGTK webview of the real shell —
not in a browser, and not against a hand-written mock. The hamburger was
found at rect `x=16 y=12 w=38 h=38` in a 420×700 viewport, hit-tested
correctly (`document.elementFromPoint` at its centre returns the `<path>`
inside the button), carrying `pointer-events: auto` inside a layer with
`pointer-events: none`, and a `click` on either the button or that `<path>`
mounted `.wl-nav-backdrop` and `.wl-nav-sheet` with real content. So the
*state write* and the *drawer render* were both healthy, and a mock could
not have told us that — every earlier screenshot of this bug was of
`preview.html`, whose DOM is not the app's.

**What was actually wrong** was the two things a screenshot of a mock can
never show, because both are relationships between elements rather than
appearances:

1. **The control was positioned against the frame, not the screen.**
   `.wl-float-layer` is `position: absolute`, and no ancestor inside the
   canvas was positioned, so its containing block was `#main` — the app
   frame. Two measured consequences: the button's `x=16` sat 2px *outside*
   the content box (which starts at `x=18`) and 8px above its top edge,
   because the layer's `padding: 12px 16px 0` was being spent inside the
   frame's padding; and the control's position was decided by an ancestor
   that also hosts the drawer, the toast and the error screen, so any
   `transform`, `filter` or `contain` on `#main` would move the one control
   the canvas has. `.wl-canvas-screen { position: relative }` makes the
   screen that owns the control the box the control is positioned against,
   and the layer's horizontal padding goes with it (`left: 0` is already
   the content edge).

2. **The panel had five writers and no owner.** `nav_open` was written by
   bare `*s.write() = …` from the hamburger, the backdrop, the close
   button, four rows and `App`'s `Escape` handler. A bare write has no
   polarity, so "open the panel" and "flip the panel" were
   indistinguishable at the point of writing, and nothing made the opener
   and the closer agree. They are now `AppCtx::open_nav` / `close_nav` /
   `toggle_nav`, the hamburger is the only caller of the first, and the
   verbs' polarity is pinned by a test rather than by a comment.

3. **`Escape` was handled twice, on one keydown.** The canvas's handler
   (bailout sheet) and `App`'s (close the drawers) are on different
   elements of the same bubbling chain, so both ran. With the control panel
   open, one press opened the bailout modal *and* closed the panel — a
   modal the user never asked for, on a canvas that had not changed, which
   from the outside is exactly "the menu does not open". The layers now
   have a declared order (`topmost_overlay` / `dismiss_topmost`: escape
   sheet → control panel → telemetry drawer) and `Escape` closes one.

4. **`Ctrl+,` could bury the panel.** The chord toggled the telemetry
   drawer unconditionally, and the drawer is a later sibling with a higher
   z-index, so summoning it over the control panel made the panel vanish
   rather than fail visibly — the same symptom a third time. It is now
   refused while any layer is up (`toggle_telemetry_allowed`).

5. **`.wl-float-menu` is deleted.** Its only rule was
   `position: static`, on a class that also carried `position: relative`
   from `.wl-circle-btn` and is never used inside a page header. A modifier
   that cannot change a declaration is a lie the next reader has to
   disprove. The same section of `SKILL.md` carried a second, older markup
   snippet naming a `wl-float-btn` class that has never existed; both are
   gone, and the skill now states the layer's containing-block rule.

**What was NOT wrong**, and is recorded so the next person does not go
looking for it again: Dioxus's event binding. `click` is a bubbling event
(`dioxus-core-types/src/bubbles.rs`), so the listener lives on `#main` and
the handler is found by walking up from the target — which is why an
inline `<svg>` child, carrying no `data-dioxus-id` of its own, still
reaches the button two levels up. The `dioxus-signals` "Copy Value created
in ScopeId(3) … used in ScopeId(0)" warning that fires on every boot is
also not it: `ScopeId(0)` is the runtime root wrapper, it is an *ancestor*
of the owning scope, and it outlives it.
