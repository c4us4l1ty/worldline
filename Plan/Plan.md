# Worldline — Master Plan (Phases)

> **Purpose:** single source of truth for *what is done* vs *what is left* to reach
> MVP + production. For future AI coding agents: work phases in order,
> check off `- [x]` as you finish, keep `cargo test --workspace` (162 tests) +
> `cargo test --manifest-path crates/wl-app/Cargo.toml` (15 tests) +
> `cargo test -p wl-ui` (10 tests) green, `clippy` warning-free, `fmt` clean.
> Record every PRD deviation in `docs/PRD-DELTAS.md` (architecture rule).
>
> **How to use:** each Phase has `Done` (do not redo) and `TODO` checkboxes.
> Finish all `TODO` in a phase before moving on, unless marked `PARALLEL-OK`.
> Reference file:line for every claim so the next agent can verify.
>
> **Repo map (2026-09-18, commit `d66f0b9`):**
> `crates/wl-core` (platform-clean: crypto/HLC/CRDT/store/engine/AI/net, 120 unit
> + 8 adversarial tests) · `crates/wl-protocol` (wire types, 6 tests) ·
> `crates/wl-relay` (Axum blind-blob relay, sqlite default / postgres feature,
> 11 + 8 tests) · `crates/wl-sync` (outbox drain + pull/apply + merge, 11 + 6
> tests) · `crates/wl-app` (Tauri v2 shell, EXCLUDED from workspace, 15 tests) +
> `crates/wl-app/ui` (Dioxus wasm, standalone workspace, 10 tests).
> Total: **187 tests green** (162 workspace + 15 shell + 10 UI).
> Docs claiming "80 tests" (`README.md:119`, `AGENTS.md`) are stale — see Phase 0.
>
> **MVP definition (updated per user changes):** open app → **Home page** (Stackelberg single directive; if no task → "No active directive."); hamburger top-left opens left slide drawer (App name top, horizontal buttons bottom: Goal creation LEFT with creation icon + contrasting/highlight, Settings RIGHT with gear icon; dimmed overlay behind drawer, drawer NOT full-screen); Settings holds 12-word phrase save + API key entry; providers strictly = Openrouter, Google, Qwen, bytez.com (Anthropic + OpenAI removed); goal creation → active directive; complete/check-in/dormant; sync. **The MVP is NOT built today.**

---

## Phase 0 — Repo hygiene & spec recovery (do FIRST, 1–2h)

### Done
- [x] Root workspace (`Cargo.toml`) = wl-core/wl-protocol/wl-relay/wl-sync; wl-app + ui deliberately standalone (webkit/gtk + wasm isolation).
- [x] `scripts/build-ui.sh` + `scripts/prune-dx-dist.sh` wired to `beforeBuildCommand`; `build.rs` fails shell build on missing dist; boot paint `#131312`; `dx build --release --debug-symbols false` pinned (PRD-DELTAS #61–62, #71–72).
- [x] `scripts/setup-linux.sh`, `scripts/setup-fedora.sh`, `scripts/smoke-native.sh`, `scripts/dx.sh` exist.
- [x] `docs/PRD-DELTAS.md` deltas #1–72 current through battle-test pass 3 (2026-09-18).
- [x] Design skill binding: `.opencode/skills/worldline/SKILL.md` (435 lines).

### TODO
- [ ] **P0-HYGIENE-1 — Restore or ratify `Plan/Frontend.md` deletion.** Worktree shows `D Plan/Frontend.md` (641-line FSM + 9:16 screen spec, still in `HEAD`). Either `git restore Plan/Frontend.md` (recommended — it is the only binding UI FSM: UNINITIALIZED → BIP39_SEED_VAULT → BYOK_KEY_STORE → MORNING_BRIEFING → DIRECTIVE_ACTIVE ⇄ ESCAPE_HATCH_MODAL → EVENING_AUDIT → DORMANT + TELEMETRY_DRAWER) or commit the removal with a delta note. Never leave a dirty delete unacknowledged.
- [ ] **P0-HYGIENE-2 — Fix stale test-count docs.** `README.md:119-123` + `AGENTS.md:6` claim "80 tests"; truth is 187 (162+15+10). Update both + `Plan/AGENT.md` (currently stale: references `cargo check -p wl-app --lib`, "Node/Webview frontend" — UI is Rust/Dioxus→wasm, shell has no lib target).
- [ ] **P0-HYGIENE-3 — Pin verification gates in this file.** Canonical gates: `cargo test --workspace` · `cargo clippy --workspace --all-targets` · `cargo fmt --all -- --check` · `cargo test/clippy --manifest-path crates/wl-app/Cargo.toml` · `cd crates/wl-app/ui && cargo test -p wl-ui` + `cargo check --target wasm32-unknown-unknown`. CI-free repo: every agent runs these before declaring a phase done.

---

## Phase 1 — P0 bugs blocking ANY MVP demo (do SECOND, must all close)

These are not polish — each one bricks a primary user path today.

- [ ] **B-001 (P0) — Provider list mismatch / Anthropic+OpenAI removal.** `crates/wl-app/src/commands.rs:853-859` allow-list must become ONLY `openrouter|google|qwen|bytez.com` (`bytez.com` added); `wl-core/src/domain/mod.rs:370` `KNOWN_PROVIDERS` must match exactly those 4; `base_for` needs `bytez.com` arm (`commands.rs:910-913`); UI provider pills (`byok.rs`/`settings.rs`) must drop Anthropic/OpenAI and show only the 4 approved. Flow broken when saved provider isn't recognized → all AI calls fail. Fix: align shell/core/UI/provider adapter matrices to the 4-only set + add `bytez` endpoint mapping + regression test (key save → master_plan with mocked `execute` succeeds for each of the 4).
- [ ] **B-002 (P0) — Manual goal is a dead end (no-AI MVP path broken).** `GoalCreate.create_manual` calls only `create_goal` then routes to Canvas (`ui/src/screens/goal_create.rs`). `create_manual_milestone` / `create_manual_directive` shell commands exist but have **zero UI callers** (grep confirms). Result: manual goal has 0 milestones → `next_runnable` = none → canvas shows "The line is clear", velocity 0/0, no path to add work. Fix (pick one, record delta): (a) milestone+directive authoring UI on GoalCreate/Canvas-empty state, or (b) auto-seed one milestone + one directive from the goal title on manual create. Must be usable with NO API key (PRD delta #6).
- [ ] **B-003 (P0) — Deletes never sync (sync correctness hole).** In-memory `wl-core/src/crdt/mod.rs:120-143` fully supports `tombstone` + resurrection, but `wl-sync/src/sync.rs:235` hardcodes `tombstone:false`, `apply_op_to_db` has no `DELETE` branch, and no `Repos` delete path enqueues a tombstone op. Deletes are local-only and resurrect on next pull. Fix: add `Repos::delete_*` tombstone writers + pull-apply `DELETE` branch + convergence test (delete on A → pull on B → row gone, stays gone after re-pull).
- [ ] **B-004 (P0) — Locked-with-lost-phrase dead end offers an impossible button.** Boot locked → `SeedVault{restore:true}` → "Create new instead" → `identity_generate` always fails `identity already exists` (`commands.rs:31`). Correct cryptographically, wrong UX. Fix: replace button copy when `identity_status.has==true` with "This install already has an identity — restore the 12 words or wipe the data dir", link to wipe instructions. Never offer an action that can never succeed.
- [ ] **B-005 (P1→P0) — MorningBrief discards IDs / assumes persistence.** `morning_briefing` returns `Vec<String>` titles; UI navigates to Canvas assuming shell persisted directives (`ui/src/screens/morning_brief.rs`). If dispatcher didn't persist (no key, offline, validation fail), Canvas shows stale/empty with no error. Fix: surface `pending/pushed/pulled` + directive count, re-fetch `current_directive` + `velocity` after briefing, show explicit empty-state CTA to GoalCreate.
- [ ] **B-006 (P1) — Evening check-in unreachable except ≥19h.** `ui/src/screens/canvas.rs:70-77` gates `Evening audit` pill on `hour>=19`; no other route to `EveningCheckIn`. Fix: always-visible entry (telemetry drawer + settings + canvas overflow) while keeping the ≥19h nudge.
- [ ] **B-007 (P1) — Hotkey unregister-before-validate kills summon.** `crates/wl-app/src/hotkey.rs:22-28` does `unregister_all()` then `on_shortcut(hotkey)`; malformed/conflicting input leaves NO hotkey until restart. Fix: validate/parse before unregistering, re-register `DEFAULT_HOTKEY (alt+space)` on failure. Also verify `capabilities/main-capability.json` (only `core:default`) needs no `global-shortcut` permission entry on Tauri v2.
- [ ] **B-008 (P1) — Unknown-table handling wedges sync (contradicts quarantine philosophy).** `apply_op_to_db` returns `Ok(())` (ignore + advance), but `validate_pull_response` rejects unknown tables with `Protocol` abort BEFORE any apply (`wl-sync/src/sync.rs`). A future-schema peer wedges sync instead of skipping. Fix: demote unknown-table to quarantine-skip (watermark + `quarantined++`), matching poison-op path. Regression test: unknown-table op → cycle completes, cursor advances.
- [ ] **B-009 (P1) — SQL LWW lacks the in-memory tertiary tie-break.** `TableState::apply` arbitrates `(HLC, device, operation_id)` (PRD-DELTAS #65), but `apply_op_to_db` guards only `hlc_timestamp < op.hlc` (except `check_ins` which adds `id<`). Identical-HLC/different-op_id races (forked data dirs, same device) converge order-dependently in SQL. Mitigated today only by boot jitter (0–255 counter bump, `repo.rs`). Fix: extend SQL guards to `(hlc, operation_id)` ordering everywhere, backfill test with forked-DB fixture.

**Exit criteria:** fresh profile can complete the full MVP path with NO API key (manual goal → directive → complete → check-in → sync across 2 app data dirs via local relay). Record a screen-by-screen transcript in the PR.

---

## Phase 2 — MVP flow completion (the "not built" gap)

### Done
- [x] FSM screens updated: Boot → Home page (single directive; no onboarding at open; "No active directive." when empty) → Hamburger drawer (left slide, dimmed full-screen overlay, drawer NOT full-screen; App name top, 2 horizontal buttons bottom: Goal creation LEFT with creation icon + contrasting/highlight, Settings RIGHT with gear icon) → Settings (12-word phrase save + API key + providers: Openrouter, Google, Qwen, bytez.com) → Canvas/DirectiveActive → CheckIn/Dormant; SeedVault/mnemonic moved to Settings (not at boot); ByokSetup removed from boot flow; GoalCreate triggered from drawer. No Anthropic/OpenAI providers anywhere.
- [x] Shell 24 commands cover identity/BYOK/authoring/canvas/settings/sync/AI/window (`main.rs:56-81`, `commands.rs:1105`). Arg shapes correct in UI (wrapped `{"settings":{…}}`, flat `reason/note`, `indices/words`, etc.).
- [x] Escape modal categorized (Blocked/Scope/Energy, `1/2/3` + `Esc`, 140-char cap); engine `bail_out` downsizes/rescales (PRD-DELTAS #31).
- [x] Single-active invariant enforced + self-healed (`set_directive_state` parks others, `enforce_single_active` after pull, `active_directive ORDER BY hlc DESC, id`).
- [x] Manual fallback without key approved (PRD delta #6); BYOK skip allowed.

### TODO
- [ ] **MVP-1 — Authoring UI (drawer button + empty-state).** Drawer must open left slide with dimmed overlay; App name at top (`DM Sans` bold `#DAD5C7`); bottom 2 buttons HORIZONTAL: Goal creation LEFT (creation icon + contrasting/highlight, e.g. `#E26D52` coral or cream `#DAD5C7`) → opens GoalCreate; Settings RIGHT (gear icon `#949087` or `#E5E2E0`) → Settings. No full-screen drawer. Empty-state on Home: "No active directive." (exact copy) — optionally with a subtle CTA linking to Goal creation or Settings. Authoring from drawer button ensures user can go: open app → see empty home → hamburger → create goal → active directive appears.
- [ ] **MVP-2 — Briefing→Canvas contract.** Return directive IDs (or count + first ID) from `morning_briefing`, or follow with `current_directive`; handle 0/1/2/3 cases explicitly (0 → GoalCreate CTA, never blank canvas).
- [ ] **MVP-3 — Check-in→velocity→dormant loop.** Refresh `velocity()` after `complete_directive` (today HUD only updates after check-in); GPS copy already exists in CheckIn; wire `estimate_adjustment` display honestly (currently computed but NOT applied — see E7; either apply it or label "observed, not yet applied").
- [ ] **MVP-4 — Sync UX.** Expose `relay_authenticate` in Settings as "Test connection" button (docstring promises it, no UI calls it); surface `SyncStats {pushed,pulled,applied,pending}` + `quarantined` + cursor in telemetry (today `SyncStatsView` omits quarantined/cursor); add "Sync now" to canvas overflow (today only via settings + telemetry force-resync). No background auto-sync yet (documented on-demand) — keep, but HUD `LOCAL/SYNCED/OFFLINE` must reflect last sync age honestly.
- [ ] **MVP-5 — Boot/home-page recovery + settings identity flow.** No onboarding at boot: `AppState::new` should NOT open SeedVault; boot routes directly to Home (single directive, empty-state "No active directive." when no goal). Identity (12-word phrase) lives in Settings: generate/verify/restore moved there; `identity_unlock` called from Settings after phrase entry, not at boot. `AppState::new` fatal on vault/open or corrupt `device_id` still requires fail-closed error screen with wipe/restore guidance — but never block boot with onboarding screens. `identity_status.vault_has_mnemonic` used to show/hide phrase-save option in Settings.
- [ ] **MVP-6 — Tauri shape audit.** Re-verify EVERY new command against the real shell (`cargo tauri dev`), not just `dx serve` — the shim (`invoke-shim.js:151`) ignores arg shapes and has known divergences (`relay_authenticate→{account_id,relay_url:null}` vs real `{account_id,expires_at}`; `sync_now→{0,0,0,0}`; mock IDs `ms-/dir-+Date.now()`).

**Exit criteria:** Phase 1 exit + AI path works for the 4 providers ONLY (`openrouter|google|qwen|bytez.com` adapters live with mocked + sandboxed HTTP); drawer opens correctly; boot routes directly to Home with "No active directive." empty-state; Settings holds phrase + API key + provider selection; `dx serve` mock and real shell agree on all 24 commands with updated provider matrix.

---

## Phase 3 — Sync + relay hardening (what's left after 3 battle passes)

### Done (PRD-DELTAS #21–33, #39–50, #55–69 — do not regress)
- [x] Fixed-width HLC `pt(20).ctr(5).dev(5)`; composite `(hlc,op_id)` cursor + per-batch persist; whole-outbox push drain + `accepted+duplicates→pushed→DELETE`; LWW-guarded pull apply incl. `app_settings`; persisted `hlc_clock` head + counter saturation; poison-op + constraint-conflict quarantine; HLC receive events; single-active self-heal; 200-batch loop bounds + `batch_limit 1–500`; transactional outbox drain; `crdt_applied` prune + drained-outbox delete; per-account `(account,op_id)` dedup; empty-batch cursor-integrity abort; relay push validation (HLC round-trip, 7-table allow-list, 256 KiB, 500/req, 100k/account); relay `spawn_blocking` (no executor block); SSRF blocklist + relay-URL allow-list; shared reqwest client; `MAX_BATCH_OPS/SEALED_B64/HEADER/REQUEST/RESPONSE` enforced.

### TODO
- [ ] **SYNC-1 (P4, characterized) — Relay push holds one txn/mutex across whole batch.** `wl-relay/tests/adversarial.rs:275-278` `defect_push_holds_global_mutex_across_whole_batch` locks the behavior. Fix: chunked commits (e.g. 100 ops/chunk) so pulls aren't convoyed behind 20k-op pushes. Keep idempotence + quota atomicity.
- [ ] **SYNC-2 — Per-account 100k op cap is a cliff.** Pushes 429 even critical writes; ops accumulate forever (no GC/TTL/eviction/compaction/VACUUM). Design + implement retention (e.g. LWW-compacted snapshot or TTL + client re-bootstrap), or at minimum a `Retry-After` + "contact admin / rotate account" error path + ops-count telemetry.
- [ ] **SYNC-3 — Postgres/SQLite divergence.** PG index `idx_ops_account_hlc(account,hlc)` lacks `(operation_id)` tail vs SQLite; `register_account LOCK TABLE` path needs prod verification via `docker compose up` + `WL_RELAY_PG_URL`; `main.rs` feature-combo fragility (both-features → SQLite silently wins; no-features → unbound `blobs`): add `compile_error!` guard.
- [ ] **SYNC-4 — Size-bound skew.** Relay `MAX_RESPONSE 4MiB` vs shell `ReqwestTransport::read_body 16MiB` vs handshake `post_json 64KiB`; `MAX_PULL_BYTES (1MiB)` defined, never referenced (`wl-protocol/src/lib.rs`). Unify + enforce + test the smallest binding limit end-to-end (unbounded strings today bloat sealed ops past 256 KiB → undrainable batch; write budgets in #64 mitigate, don't eliminate).
- [ ] **SYNC-5 — Cursor fidelity.** `SyncStats.cursor` returns HLC only, drops `op_id`; callers can't resume exactly. Return `(cursor_hlc, cursor_op_id)` through `sync_now` → telemetry.
- [ ] **SYNC-6 — Forward-compat policy.** Decide once: unknown-table/oversize-sealed = quarantine-skip (resilient) vs abort (strict). Today pull-validator aborts, apply ignores — pick quarantine (see B-008) and document.
- [ ] **SYNC-7 — Deferred crypto hardening (documented, not fixed).** AEAD AAD is only `table:record` — bind `operation_id`/HLC (needs migration + version gate); HLC arrival-sequence cutover; `operation_id` global-uniqueness squat (accepted: UUID prediction infeasible, but re-audit); DNS-rebinding TOCTOU (capped by E2EE blindness — document residual).
- [ ] **SYNC-8 — Opaque sessions die on restart; no TLS.** In-memory `HashMap` (clients recover via 401→re-handshake — keep); production needs reverse-proxy TLS (not in repo — add example Caddy/nginx + `WL_RELAY_ADDR` docs); unauthenticated registration (10k-account cap only — rate-limit by IP or invite code before public deploy).

---

## Phase 4 — Core engine + store correctness (Tier-3 + lifecycle)

### Done
- [x] `Engine::{activate_next,current,complete,bail_out,check_in}` + `RecoveryAction` + `velocity::compute` (EWMA α=0.4, 14-day window, `adjustment=0.6+0.4*ratio` floored 0.6); progressive phases (threshold 30m, auto-split 5m+rest, sum-rewrite, 2× band); `reschedule` resets to phase 1 + rescales; write-boundary budgets (titles ≤500, desc ≤4000, notes ≤2000, models ≤256, 1–1440m, strict `YYYY-MM-DD`, 140-char bailout); transactional row+outbox+HLC commits; `check_text/date/minutes` validators.

### TODO
- [ ] **CORE-1 (E8) — `blocked` has no unblock path; `skipped` never completes a milestone.** `engine/mod.rs:173-202,330`. Design the state machine: `blocked → (unblock → queued/active | escalate → bailout)`; define what `skipped` does to milestone progress (counts 0 today — keep, but surface "velocity adjustment" + offer requeue). Add `unblock_directive` command + UI affordance. Characterization guard exists — replace with real behavior + tests.
- [ ] **CORE-2 (E7) — `estimate_adjustment` computed, never applied.** `engine/velocity.rs:82` → `commands.rs:584-602` → `checkin.rs:59` displays but future estimates ignore it. Either apply (with 60% floor, PRD delta #16) at `create_directive` time or rename to `observed_adjustment` + delta note. Don't display a number that does nothing.
- [ ] **CORE-3 — No `set_goal_status`; multiple `active` goals accumulate.** `GoalStatus::{Active,Achieved,Archived}` exists, `Repos` has no setter (tests use raw SQL); `persist_plan` always inserts new `active` goal; `active_goal() ORDER BY hlc LIMIT 1` is arbitrary. Add `set_goal_status` + archive-previous-on-new-plan + test.
- [ ] **CORE-4 — `identity_config` allows N rows.** PK=`public_key` + `ON CONFLICT(public_key)` permits multiples; `identity() LIMIT 1` arbitrary; `set_mnemonic_verified` updates ALL rows. Make singleton (`id=1`) or delete-on-restore + test.
- [ ] **CORE-5 — CRDT poison-op mutates state on reject.** `crdt/mod.rs:120-143` inserts `last_op` + removes `tombstone` BEFORE `resurrect_ok`/`fields` checks; field-less upsert mutates then returns `false`. Fix: validate `fields.is_some()` before touching `last_op/tombstones` + regression test.
- [ ] **CORE-6 — `current_phase_minutes` fail-closed with no repair.** Progressive `total>1` + missing phase row → `Invalid("current phase missing")` breaks `activate_next/complete`/HUD. Add repair (rebuild phases from estimate) or honest "corrupt directive — requeue" path.
- [ ] **CORE-7 — Gaps.** `save_settings` never validates `relay_url` (relies on shell pre-validate — move `net::validate_relay_url` into `Repos::save_settings`); `enqueue_outbox` has no payload-size cap (relay 256 KiB — enforce at enqueue); `validate_plan` message says "2–5 milestones" but code allows 1 (`ai/dispatch.rs` — fix message OR require `len>=2`); Tier-2 `persist_briefing` has no same-day dedup (double-run duplicates today — add idempotency key); `reschedule_directive as i64` cast (safe ≤2M today — add debug_assert); prompts embed raw user text (pre-check caps exist in dispatcher — enforce in `persist_*` too); `Engine` reaches into `repos.conn` pub field (`engine/mod.rs:283,302,327` — add accessor); `None`-identity silent fork (documented — add debug_assert/log when unlocked-but-None).

---

## Phase 5 — Shell robustness (Tauri + vault + IPC)

### Done
- [x] `AppState {repos, identity: Mutex<Option<Identity>>, vault, relay_token, relay_url}`; identity-mutex serialization; `with_identity/with_identity_opt`; vault `0600`/`0700` hardening + `save_mnemonic` rollback; snapshot KDF work-factor 0 (strong key, PRD delta #20); Stronghold NOT registered as plugin (PRD delta #21); `pub(crate)` commands; 401→re-handshake→retry once; session TTL 1h; sync I/O on `spawn_blocking` only; 15 shell tests (vault round-trip/perms, session scoping, live-relay handshake, snapshot-failure atomicity).

### TODO
- [ ] **SHELL-1 — Mutex-poison fail-stop everywhere.** `conn.lock().unwrap()` (~30 sites `repo.rs` + `engine` + `sync` + `auth` + `store`) and `commands.rs:30,69,107,164,186,331,463,655,658…` panic on poison instead of recovering. Map to `StoreError`/`ShellError` via `lock().unwrap_or_else(|p| p.into_inner())` or graceful restart prompt. At minimum, shell + relay must not fail-stop a long-lived process on one bad holder.
- [ ] **SHELL-2 — `spawn_blocking` discipline is footgun-by-design.** `relay::handshake` + `ReqwestTransport::post` `block_on` panics if called from async context (`relay.rs:10-16`). Either refactor to pure-async transport or add `debug_assert!(!in_async_context)` + lint + doc. Tracked deferred — close it.
- [ ] **SHELL-3 — Vault API-key write lacks rollback** (mnemonic has it, keys don't). Add same commit-failure restore for `save_api_key` + test.
- [ ] **SHELL-4 — API-key Zeroizing residual.** Headers + reqwest internals hold short-lived copies (documented). Re-audit; clear what can be cleared; keep residual note current.
- [ ] **SHELL-5 — Boot/home-page flow (no onboarding).** `AppState::new` must NOT trigger `SeedVault`; boot effect routes to `Screen::Canvas` (or empty-state home) regardless of identity state; identity is unlocked via Settings after user enters phrase. `device_id_for` repair UI still needed for corrupt device IDs; `identity_unlock` called from Settings, not boot. `read_body` underflow-panic path still needs fail-closed `Err` (`relay.rs:97-115`). `unwrap_or_default` boot effect must distinguish "loading" (no identity yet, fine) from "locked with no vault file" (show error screen with wipe/restore link).
- [ ] **SHELL-6 — Vault is `0600` file, not OS keychain.** `vault.rs` boundary already shaped for migration (PRD delta #19). Migrate to keyring/Stronghold-OS-backend before public release; add vault-wide key purge (today per-provider `delete_api_key` only).

---

## Phase 6 — UI/UX completion + design-system compliance

### Done (frontend pass 2026-09-16 + battle-test 3 — do not regress)
- [x] Tokens (`#131312/#20201F/#DAD5C7/#E26D52`, light `#F4F0E8/#ECE6DA`), fonts bundled (DM Sans/Doppio One, no CDN), 420×747 fixed, single-directive canvas, categorized escape, GPS "velocity adjustment" copy, no streaks/red/future-list, secrets cleared post-verify/restore/save, key-probe resubscribe, pin-to-top persist+revert, double-submit guards, positional backup verify, `(hlc,id)` ordering, HUD tick gated on Canvas, `Signal<String>` sync_status, reduced-motion + focus ring.

### TODO (bugs + drifts — all in `crates/wl-app/ui/src`)
- [ ] **UI-1 — Type mismatch masked.** Shell always returns `DirectiveView` (idle `state:"idle",id:""`); UI invokes as `Option<DirectiveView>` relying on `active_directive` filter. Works, masks errors. Fix: invoke as `DirectiveView` + explicit `is_idle()` check.
- [ ] **UI-2 — EscapeModal inconsistency.** Backdrop click → `confirming` ("Keep directive?") while `Esc` closes instantly; recovery string (`downsized:15/advanced:…/low-cognitive`) discarded, generic toast only. Unify: Esc == backdrop (friction preserved both), toast shows actual recovery ("Resized to 15m · back on the line tomorrow").
- [ ] **UI-3 — Settings staleness.** `local` snapshot at mount, never resyncs; theme applies before Save (diverges on abandon); pin bypasses Save while hotkey doesn't. Fix: resync on focus, Save-or-revert theme, consistent persist model, hotkey format hint + validation.
- [ ] **UI-4 — Drawer design + home-page discovery.** Drawer must be left-side slide-out with full-screen dimmed overlay behind it (`rgba(19,19,18,0.85)` or similar); drawer itself is NOT full-screen (e.g. ~65% width, rounded right edge, `#20201F` card background). Top: App name (`DM Sans` bold, `#DAD5C7`). Bottom: 2 buttons stacked HORIZONTALLY (side by side): Goal creation LEFT (creation icon + contrasting/highlight — e.g. coral `#E26D52` or cream `#DAD5C7` pill, `#131312` icon); Settings RIGHT (gear icon `#949087`, `#20201F` pill). Only icons, no text labels required (but accessible `aria-label` needed). Drawer closes on Esc / backdrop click / button tap. Empty-state on Home: "No active directive." — minimal, centered or HUD-style; optional subtle text link to hamburger → create goal. No backlog list on canvas (Stackelberg preserved).
- [ ] **UI-5 — Copy + provider + drawer icon bugs.** Provider pills must show ONLY Openrouter, Google, Qwen, bytez.com (drop Anthropic/OpenAI references everywhere: `byok.rs`, `settings.rs`, `goal_create.rs`, `commands.rs`, `ai/dispatch.rs`, `wl-core/domain.rs`). Drawer icon buttons: Goal creation must use a creation/relevant icon (not text); Settings must use gear icon (`#949087`). Highlight Goal button with contrasting color (`#E26D52` coral or `#DAD5C7` cream). Telemetry claims `Argon2id · 128-bit salt` — fix to Stronghold ChaCha. Settings "Keys never touch the DOM" — reword. `rgba(0,0,0,…)` shadows — allowlist or replace. Skill §7 card `#1C1C1B` vs code `#20201F` — fix doc. MorningBrief 3-title stack — add PRD-DELTAS note. Dead CSS `.wl-btn-coral/.wl-phase-list` — delete. Unify provider component.
- [ ] **UI-6 — Telemetry honesty.** HLC head / WAL size render `— (no shell command)` (correct — don't fabricate). Decide: add `sync_debug_info` command (HLC head, cursor `(hlc,op_id)`, outbox depth, WAL bytes, quarantined count) or keep `—` permanently. Either way, stop showing live-geometry as static `420×747`.
- [ ] **UI-7 — A11y + i18n.** Focus-trap escape modal, `aria-live` toast, timer `aria-label`, keyboard map help (`?`). No i18n in v1 — record.

---

## Phase 7 — AI BYOK completion (Tier-1/2 + Tier-3 wiring)

### Done
- [x] Provider adapters for Openrouter, Google, Qwen, bytez.com (`ProviderAdapter` variants updated to 4 only); redacted Debug + `Zeroizing` key (moved, not cloned); `temperature:0.4 + response_format:json_object` / `max_tokens:4096`; `strip_fences/extract_json_block/validate_response_size 256KiB/validate_text 16KiB`; `validate_plan (1..5 milestones, ≤32 dirs/milestone)` + `validate_directive` (phase-sum 2× band, >30m requires phases); `persist_plan/persist_briefing`; `AiDispatcher::{master_plan,morning_briefing}` with injected `execute` (platform-clean); pre-HTTP validation (`MAX_AI_CONTEXT 16K`, `MAX_AI_CONSTRAINTS 8K`); model IDs user-configurable (`tier1_model/tier2_model`), 4-provider allow-list (`openrouter|google|qwen|bytez.com`).

### TODO
- [ ] **AI-1 — AI batch atomicity deferred.** `persist_plan` writes goal+milestones+directives non-atomically (partial plan on crash). Wrap in one `Repos` transaction or two-phase (validate-all → commit-all) + test (kill mid-persist → no partial goal).
- [ ] **AI-2 — Model/provider matrix (4 providers ONLY).** After B-001, verify adapters for Openrouter, Google, Qwen, bytez.com live. `base_for` must map all 4; drop Anthropic/OpenAI adapter code + `base_for` arms; `ProviderAdapter` variants updated to the 4; `vault_api_key` allow-list = the 4; `KNOWN_PROVIDERS` = the 4. Mocked + sandboxed-live tests for each.
- [ ] **AI-3 — Briefing context (4 providers).** Tier-2 prompt gets active milestone + velocity JSON + constraints (`prompt::tier2_*`); providers ONLY Openrouter/Google/Qwen/bytez; distinguish missing-key vs offline errors in UI; finish settings-first identity flow (phrase + API key saved in Settings).
- [ ] **AI-4 — Cost guards.** Pre-HTTP validation exists — add per-day call counter + "last plan cost" estimate display before billable hop. Never auto-retry billable calls on 5xx without user confirm.

---

## Phase 8 — Verification, packaging & production readiness

### Done
- [x] 187 green; clippy clean (workspace + wl-app); fmt clean; `smoke-native.sh` pixel-asserts release window (stddev 104, canvas 0.26, KDE/Wayland); idle 0.01% CPU, 151 MiB RSS (claimed 2026-09-17 — re-verify, not re-run here).

### TODO
- [ ] **SHIP-1 — Human-hardware gates (cannot be done in sandbox).** `bash scripts/setup-linux.sh` → `cd crates/wl-app && cargo tauri build` → `scripts/smoke-native.sh` → attach screenshot + stddev/canvas numbers. `docker compose up -d` → Postgres relay (`WL_RELAY_PG_URL`) → two-device sync transcript. `Alt+Space` WM-collision note (Linux menu conflict — document rebind). All three before calling MVP "done".
- [ ] **SHIP-2 — Security pre-release.** OS-keychain migration (SHELL-6); PASETO `v4.local` wrapper behind same `Bearer` header (PRD delta #4); per-device keys/X25519 v2 scoping (dropped in v1 delta #3 — record v2 plan, no code); metadata-host SSRF re-audit incl. DNS-rebinding residual; `cargo audit` + `cargo deny` pass.
- [ ] **SHIP-3 — Perf budgets.** Canvas re-render <16ms on outbox update; zero idle GPU (no infinite glow/blur); release wasm ≤1 MB (prune guard); relay p95 push/pull latency SLO + 100k-cap load test (see P4).
- [ ] **SHIP-4 — Mobile-ready (deferred, approved delta #8).** No Android/iOS scaffolding in v1; verify `wl-core` stays platform-clean (no tokio/Tauri/I-O beyond SQLite — `grep` gate in CI-less checklist); `tauri android/ios init` smoke when toolchains exist.
- [ ] **SHIP-5 — Docs.** PRD (missing from `docs/` — only deltas exist; recover or declare deltas canonical); `README.md` verification-status table refresh (stale "80 pass"); user runbook (install → open app directly to Home (no onboarding) → hamburger drawer → Settings → save 12-word phrase + enter API key for one of 4 providers [Openrouter/Google/Qwen/bytez] → exit Settings → drawer → create goal → active directive on canvas → check-in → sync); drawer design spec (left slide, dimmed overlay, App name top, 2 horizontal icon buttons); operator runbook (relay env `WL_RELAY_DB/ADDR/PG_URL`, quotas, TLS proxy, backups — relay blobs are opaque, loss = data loss).

---

## Appendix A — Bug registry (quick index)

| ID | Severity | Location | One-liner |
|----|----------|----------|-----------|
| B-001 | P0 | `wl-app/src/commands.rs:853` vs `ui/.../byok.rs:14` vs `wl-core/.../domain.rs:370` | Providers not aligned to 4-only (Openrouter/Google/Qwen/bytez); Anthropic/OpenAI still in adapter/core/UI |
| B-002 | P0 | `ui/.../goal_create.rs` | Manual goal dead end, 0 milestone/directive authoring UI |
| B-003 | P0 | `wl-sync/src/sync.rs:235` | Deletes never sync (no tombstone path) |
| B-004 | P1 | `ui/.../seed_vault.rs` + `commands.rs:31` | Impossible "Create new" button when locked |
| B-005 | P1 | `ui/.../morning_brief.rs` | Briefing discards IDs, assumes persistence |
| B-006 | P1 | `ui/.../canvas.rs:70` | Check-in gated ≥19h, no other route |
| B-007 | P1 | `wl-app/src/hotkey.rs:22` | Unregister-before-validate kills summon |
| B-008 | P1 | `wl-sync/src/sync.rs` validate vs apply | Unknown-table wedges sync |
| B-009 | P1 | `wl-sync/src/sync.rs` apply guards | SQL LWW missing op_id tie-break |
| CORE-1 (E8) | P1 | `wl-core/src/engine/mod.rs:173` | `blocked` no unblock; `skipped` never completes milestone |
| CORE-2 (E7) | P2 | `wl-core/src/engine/velocity.rs:82` | Adjustment displayed, never applied |
| CORE-3 | P1 | `wl-core/src/store/repo.rs` goals | No `set_goal_status`, multi-active goals |
| CORE-4 | P2 | `wl-core/src/store/repo.rs` identity | N-row `identity_config` |
| CORE-5 | P2 | `wl-core/src/crdt/mod.rs:120` | Poison-op mutates on reject |
| CORE-6 | P2 | `wl-core/src/engine/mod.rs` phases | Missing-phase fail-closed, no repair |
| CORE-7 | P2 | various (see Phase 4) | Settings URL / outbox cap / plan-count / dedup gaps |
| SYNC-1 (P4) | P2 | `wl-relay/src/store.rs` | Push holds global mutex/txn whole batch |
| SYNC-2 | P2 | `wl-relay/src/store.rs` quotas | 100k cap cliff, no GC |
| SYNC-3 | P2 | `wl-relay` pg vs sqlite | Index divergence, feature-combo fragility |
| SYNC-4 | P2 | `wl-protocol/src/lib.rs` | Size-bound skew, dead `MAX_PULL_BYTES` |
| SHELL-1 | P2 | `repo.rs` + `commands.rs` | Mutex-poison fail-stop |
| SHELL-2 | P2 | `wl-app/src/relay.rs:10` | `spawn_blocking` footgun |
| UI-1…UI-7 | P2 | `ui/src/...` | Idle-coercion, modal, staleness, discovery, copy, telemetry, a11y |
| AI-1 | P2 | `wl-core/src/ai/dispatch.rs` | Plan persist not atomic |
| HYGIENE | P0 | `Plan/Frontend.md`, `README.md`, `Plan/AGENT.md` | Deleted spec, stale counts |

## Appendix B — Design constraints (never violate; from skill + AGENTS.md)

- 420×747 non-maximizable; exactly ONE directive on canvas; no future-task lists.
- `#131312/#20201F/#DAD5C7`, coral `#E26D52` timer/beacon ONLY; no `#000/#FFF`, no red failure, no streaks. Skips = "velocity adjustments" (GPS metaphor).
- Escape requires reason categorization before unmount. `⌘/Ctrl+Enter` complete, `Esc` bailout, `Ctrl+,` telemetry.
- Drawer: left slide (`~65%` width), full-screen dimmed overlay (`rgba(19,19,18,0.85)`), NOT full-screen; top App name (`DM Sans` bold `#DAD5C7`), bottom 2 buttons HORIZONTAL: Goal creation LEFT (creation icon + contrasting/highlight), Settings RIGHT (gear icon). Only icons; accessible labels. Closes on Esc / backdrop / tap.
- Empty-state (no directive): "No active directive." — no red, no streak, no backlog list. Optional subtle link to hamburger → create goal.
- Secrets never touch SQLite/DOM/relay (single mandated mnemonic display excepted, cleared after). Relay never sees plaintext (seal `table:record` AAD). HLC on every mutation. `wl-core` stays platform-clean. Deltas → `docs/PRD-DELTAS.md`.

## Appendix C — Suggested agent execution order

1. Phase 0 (restore spec, fix docs) — 1 PR.
2. Phase 1 B-001 + B-002 + B-005 + B-006 (MVP unblock, UI+shell) — 1–2 PRs, verify manual + AI paths in `cargo tauri dev`.
3. Phase 1 B-003 + B-008 + B-009 (sync correctness) — PRs with convergence tests.
4. Phase 1 B-004 + B-007 + Phase 2 MVP-1…MVP-6 — UI completion PRs.
5. Phase 4 CORE-1…CORE-7 (engine/store) — unit-test PRs.
6. Phase 3 SYNC-1…SYNC-8 (relay/prod) — load + PG verification.
7. Phase 5 + 6 + 7 (shell/UI/AI polish) — parallel-OK after 2–4.
8. Phase 8 SHIP-1…SHIP-5 (human-hardware + docs) — final sign-off, tag MVP.
