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
