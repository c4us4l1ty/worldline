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
    authoring UI is deferred to Phase-2 MVP-1; the
    `create_manual_milestone` / `create_manual_directive` shell
    commands already exist for it.

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
