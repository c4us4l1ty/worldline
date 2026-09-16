//! Tauri command surface — thin bridge: Dioxus UI → shell → wl-core.
//! Secrets (mnemonic, API keys) flow native-side only; per skill §6.3
//! they never enter DOM dataset attributes.

use serde::Serialize;
use tauri::State;
use zeroize::Zeroizing;

use wl_core::ai::dispatch::{AiDispatcher, ProviderAdapter};
use wl_core::crypto::identity::Identity;
use wl_core::domain::*;
use wl_core::engine::{Engine, EngineOutcome, RecoveryAction};
use wl_core::store::repo::Repos;

use crate::app_state::AppState;
use crate::error::{ShellError, ShellResult};
use crate::relay;

// ---------------------------------------------------------------------------
// Identity (US-1)
// ---------------------------------------------------------------------------

/// Generates a fresh 12-word identity. Returns the mnemonic ONCE for
/// onboarding display; it is persisted ONLY in the Stronghold vault
/// (hardware-backed), never in SQLite.
#[tauri::command]
pub(crate) async fn identity_generate(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<GeneratedIdentity> {
    let identity = Identity::generate().map_err(|_| ShellError::BadMnemonic)?;
    let phrase = identity.phrase().to_string();
    let account_id = identity.account_id_hex();
    // Backup challenge positions: 3 distinct random indices into the
    // 12 words (Item 6 — no longer deterministic).
    let mut rng = rand::thread_rng();
    let verify_indices: Vec<usize> = rand::seq::index::sample(&mut rng, 12, 3).into_vec();
    state
        .repos
        .insert_identity(&identity, false, &verify_indices)?;
    // Vault first: a crash between memory and disk must not lose the
    // only recoverable copy shown to the user once.
    state
        .vault
        .save_mnemonic(&phrase)
        .map_err(|e| ShellError::Vault(e.to_string()))?;
    *state.identity.lock().unwrap() = Some(identity);
    // TODO(stronghold): persist mnemonic to the vault at onboarding
    // completion instead of keeping it process-resident. v0.1 keeps
    // the identity in memory; the phrase returned here is shown once.
    Ok(GeneratedIdentity {
        phrase,
        account_id,
        verify_indices,
    })
}

#[derive(Serialize)]
pub struct GeneratedIdentity {
    phrase: String,
    account_id: String,
    /// 3 distinct random word positions for the backup challenge.
    verify_indices: Vec<usize>,
}

/// Verifies the 3-word backup challenge POSITIONALLY (the word at
/// challenge position i must match the phrase word at position i) and
/// marks the mnemonic verified (PRD §3.1 onboarding step). Membership
/// anywhere in the phrase is not verification — order is the check.
#[tauri::command]
pub(crate) async fn identity_verify_backup(
    state: State<'_, std::sync::Arc<AppState>>,
    indices: Vec<usize>,
    words: Vec<String>,
) -> ShellResult<bool> {
    let guard = state.identity.lock().unwrap();
    let Some(id) = guard.as_ref() else {
        return Err(ShellError::Locked);
    };
    // Challenge positions come from persisted identity_config — never
    // from the caller — so a hostile UI payload can't pick its own.
    let stored = state
        .repos
        .identity()?
        .ok_or(ShellError::Locked)?
        .verify_indices;
    let indices: Vec<usize> = if !stored.is_empty() {
        stored
    } else {
        // Legacy rows predate persisted indices: the UI's positions
        // are the best available (recorded for future re-checks).
        if indices.is_empty() {
            return Ok(false);
        }
        indices
    };
    let ok = id.verify_backup_words(&indices, &words);
    if ok {
        state.repos.set_mnemonic_verified(true)?;
    }
    Ok(ok)
}

/// Restores an identity from a 12-word mnemonic (account recovery).
#[tauri::command]
pub(crate) async fn identity_restore(
    state: State<'_, std::sync::Arc<AppState>>,
    phrase: String,
) -> ShellResult<String> {
    let identity = Identity::from_phrase(&phrase).map_err(|_| ShellError::BadMnemonic)?;
    let account_id = identity.account_id_hex();
    // Fresh challenge positions for the re-verified backup check.
    let mut rng = rand::thread_rng();
    let verify_indices: Vec<usize> = rand::seq::index::sample(&mut rng, 12, 3).into_vec();
    state
        .repos
        .insert_identity(&identity, true, &verify_indices)?;
    state
        .vault
        .save_mnemonic(&phrase)
        .map_err(|e| ShellError::Vault(e.to_string()))?;
    *state.identity.lock().unwrap() = Some(identity);
    Ok(account_id)
}

/// Whether an identity exists (onboarding gate).
#[tauri::command]
pub(crate) async fn identity_status(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<IdentityStatus> {
    let has = state.repos.identity()?.is_some();
    let unlocked = state.identity.lock().unwrap().is_some();
    let vault_has_mnemonic = state.vault.has_mnemonic();
    Ok(IdentityStatus {
        has,
        unlocked,
        vault_has_mnemonic,
    })
}

#[derive(Serialize)]
pub struct IdentityStatus {
    has: bool,
    unlocked: bool,
    vault_has_mnemonic: bool,
}

/// Restores the in-memory identity from the vault copy (post-restart
/// unlock). Fails closed when nothing was ever stored.
#[tauri::command]
pub(crate) async fn identity_unlock(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<String> {
    let phrase = state
        .vault
        .get_mnemonic()
        .map_err(|e| ShellError::Vault(e.to_string()))?;
    let identity = Identity::from_phrase(phrase.as_str()).map_err(|_| ShellError::BadMnemonic)?;
    let account_id = identity.account_id_hex();
    // Upsert: the public half may already exist from a previous boot.
    // Keep the persisted challenge positions AND the verified flag (the
    // onboarding verification actually happened) — overwriting indices
    // with [] would drop the positional challenge and reopen the S2
    // membership-bypass via the legacy caller-supplied-indices path.
    let (verified, indices) = match state.repos.identity()? {
        Some(cfg) if cfg.public_key == account_id => {
            (cfg.bip39_mnemonic_verified, cfg.verify_indices)
        }
        _ => (true, Vec::new()),
    };
    state.repos.insert_identity(&identity, verified, &indices)?;
    *state.identity.lock().unwrap() = Some(identity);
    Ok(account_id)
}

// ---------------------------------------------------------------------------
// BYOK key management (Item 2) — secrets enter via these commands and
// never leave the vault except into an outbound HTTPS request body.
// ---------------------------------------------------------------------------

/// Stores (or replaces) a provider API key in the vault.
#[tauri::command]
pub(crate) async fn set_api_key(
    state: State<'_, std::sync::Arc<AppState>>,
    provider: String,
    key: String,
) -> ShellResult<bool> {
    if key.trim().is_empty() {
        return Err(ShellError::Invalid("empty API key".into()));
    }
    state
        .vault
        .save_api_key(&provider, key.trim())
        .map_err(|e| ShellError::Vault(e.to_string()))?;
    Ok(true)
}

/// Whether a key is stored for the provider (presence only — the key
/// itself is never readable through commands).
#[tauri::command]
pub(crate) async fn has_api_key(
    state: State<'_, std::sync::Arc<AppState>>,
    provider: String,
) -> ShellResult<bool> {
    state
        .vault
        .has_api_key(&provider)
        .map_err(|e| ShellError::Vault(e.to_string()))
}

/// Removes a stored provider key.
#[tauri::command]
pub(crate) async fn delete_api_key(
    state: State<'_, std::sync::Arc<AppState>>,
    provider: String,
) -> ShellResult<bool> {
    state
        .vault
        .delete_api_key(&provider)
        .map_err(|e| ShellError::Vault(e.to_string()))
}

// ---------------------------------------------------------------------------
// Goal / milestone / directive authoring (manual fallback)
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) async fn create_goal(
    state: State<'_, std::sync::Arc<AppState>>,
    title: String,
    description: Option<String>,
    target_date: Option<String>,
) -> ShellResult<GoalJson> {
    // Identity is optional here: logged-out goal drafts still work, but
    // only an unlocked vault write-throughs to the outbox.
    let g = state.with_identity_opt(|identity| {
        state
            .repos
            .create_goal(
                &title,
                description.as_deref(),
                target_date.as_deref(),
                identity,
            )
            .map_err(ShellError::from)
    })?;
    Ok(goal_json(&g, &state.repos))
}

fn goal_json(g: &Goal, repos: &Repos) -> GoalJson {
    let milestones = repos.milestones_for_goal(&g.id).unwrap_or_default();
    let total = milestones.len();
    let done = milestones
        .iter()
        .filter(|m| m.status == MilestoneStatus::Completed)
        .count();
    GoalJson {
        id: g.id.clone(),
        title: g.title.clone(),
        target_date: g.target_date.clone(),
        milestone_done: done,
        milestone_total: total,
    }
}

#[derive(Serialize, Clone)]
pub struct GoalJson {
    pub id: String,
    pub title: String,
    pub target_date: Option<String>,
    pub milestone_done: usize,
    pub milestone_total: usize,
}

#[tauri::command]
pub(crate) async fn create_manual_milestone(
    state: State<'_, std::sync::Arc<AppState>>,
    goal_id: String,
    title: String,
    description: Option<String>,
) -> ShellResult<String> {
    let next = state
        .repos
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT COALESCE(MAX(order_index) + 1, 0) FROM milestones WHERE goal_id = ?1",
            [&goal_id],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| ShellError::Store(e.into()))?;
    let m = state.with_identity_opt(|identity| {
        state
            .repos
            .create_milestone(&goal_id, &title, description.as_deref(), next, identity)
            .map_err(ShellError::from)
    })?;
    Ok(m.id)
}

#[tauri::command]
pub(crate) async fn create_manual_directive(
    state: State<'_, std::sync::Arc<AppState>>,
    milestone_id: String,
    title: String,
    execution_context: Option<String>,
    estimated_minutes: i64,
    scheduled_for_date: Option<String>,
) -> ShellResult<String> {
    let date = scheduled_for_date.unwrap_or_else(today_local);
    let phases: Vec<(String, Option<String>, i64)> =
        if estimated_minutes > Directive::PROGRESSIVE_THRESHOLD_MINUTES {
            vec![
                (format!("{title} — open and start (5 min)"), None, 5),
                (
                    format!("{title} — deep execution"),
                    None,
                    (estimated_minutes - 5).max(10),
                ),
            ]
        } else {
            vec![]
        };
    let total = if phases.is_empty() {
        1
    } else {
        phases.len() as i64
    };
    let d = state.with_identity_opt(|identity| {
        state
            .repos
            .create_directive(
                &milestone_id,
                &title,
                execution_context.as_deref(),
                estimated_minutes,
                total,
                &date,
                &phases,
                identity,
            )
            .map_err(ShellError::from)
    })?;
    Ok(d.id)
}

// ---------------------------------------------------------------------------
// Stackelberg canvas (US-3)
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct DirectiveView {
    pub directive_id: String,
    pub milestone_id: String,
    pub title: String,
    pub instruction: Option<String>,
    pub phase: Option<(i64, i64)>,
    pub estimated_minutes: i64,
    pub state: String,
    pub milestone_title: Option<String>,
}

fn outcome_view(out: &EngineOutcome, repos: &Repos) -> DirectiveView {
    match out {
        EngineOutcome::DirectiveActive {
            directive_id,
            milestone_id,
            phase,
            estimated_minutes,
        } => {
            let (title, instruction, state) = directive_details(repos, directive_id);
            let milestone_title = milestone_title(repos, milestone_id);
            DirectiveView {
                directive_id: directive_id.clone(),
                milestone_id: milestone_id.clone(),
                title,
                instruction,
                phase: *phase,
                estimated_minutes: *estimated_minutes,
                state,
                milestone_title,
            }
        }
        _ => DirectiveView {
            directive_id: String::new(),
            milestone_id: String::new(),
            title: "All clear for today".into(),
            instruction: Some(
                "The queue is empty. Check in this evening or plan tomorrow's goal.".into(),
            ),
            phase: None,
            estimated_minutes: 0,
            state: "idle".into(),
            milestone_title: None,
        },
    }
}

fn directive_details(repos: &Repos, id: &str) -> (String, Option<String>, String) {
    if let Some(d) = repos.directive(id).unwrap_or(None) {
        let phases = repos.phases_for_directive(id).unwrap_or_default();
        let instruction = phases
            .iter()
            .find(|p| p.step == d.progressive_step)
            .and_then(|p| p.instruction.clone())
            .or_else(|| d.execution_context.clone());
        (d.title, instruction, d.state.as_str().into())
    } else {
        ("Unknown".into(), None, "unknown".into())
    }
}

fn milestone_title(repos: &Repos, milestone_id: &str) -> Option<String> {
    repos
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT title FROM milestones WHERE id = ?1",
            [milestone_id],
            |r| r.get::<_, String>(0),
        )
        .ok()
}

#[tauri::command]
pub(crate) async fn current_directive(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<DirectiveView> {
    let today = today_local();
    // E4: the read path previously built Engine::new(repos, None), so a
    // canvas-load activation mutated state to `active` without an outbox
    // op — peers never learned and the one-active invariant diverged.
    // Pass the unlocked identity through when available so activation
    // write-throughs like every other transition (still None while the
    // vault is locked, where there is nothing to sync with anyway).
    let out = state.with_identity_opt(|identity| {
        let e = Engine::new(&state.repos, identity);
        e.current(&today).map_err(ShellError::from)
    })?;
    Ok(outcome_view(&out, &state.repos))
}

#[tauri::command]
pub(crate) async fn complete_directive(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<DirectiveView> {
    let today = today_local();
    let out = state.with_identity(|identity| {
        let e = Engine::new(&state.repos, Some(identity));
        e.complete(&today)?;
        e.current(&today).map_err(ShellError::from)
    })?;
    Ok(outcome_view(&out, &state.repos))
}

#[tauri::command]
pub(crate) async fn bail_out(
    state: State<'_, std::sync::Arc<AppState>>,
    reason: String,
    note: Option<String>,
) -> ShellResult<BailoutOutcome> {
    let reason = BailoutReason::from_str(&reason)
        .ok_or_else(|| ShellError::Invalid("unknown bailout reason".into()))?;
    let today = today_local();
    let out = state.with_identity(|identity| {
        let e = Engine::new(&state.repos, Some(identity));
        e.bail_out(&today, reason, note.as_deref())
            .map_err(ShellError::from)
    })?;
    match out {
        EngineOutcome::BailedOut {
            directive_id,
            reason,
            recovery,
        } => Ok(BailoutOutcome {
            directive_id,
            reason: reason.as_str().into(),
            recovery: recovery.as_ref().map(recovery_summary),
        }),
        _ => Ok(BailoutOutcome {
            directive_id: String::new(),
            reason: reason.as_str().into(),
            recovery: Some("idle".into()),
        }),
    }
}

#[derive(Serialize)]
pub struct BailoutOutcome {
    pub directive_id: String,
    pub reason: String,
    pub recovery: Option<String>,
}

/// Human-readable recovery summary for the UI. (A `Display` impl is
/// impossible here: orphan rule — `RecoveryAction` lives in wl-core.)
fn recovery_summary(r: &RecoveryAction) -> String {
    match r {
        RecoveryAction::DownsizeAndRequeue { new_minutes, .. } => {
            format!("downsized:{new_minutes}")
        }
        RecoveryAction::AdvanceUnblocked { next_directive_id } => {
            format!("advanced:{next_directive_id}")
        }
        RecoveryAction::LowCognitiveTask { .. } => "low-cognitive".into(),
    }
}

// ---------------------------------------------------------------------------
// Check-in & velocity (PRD §5.4)
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) async fn check_in(
    state: State<'_, std::sync::Arc<AppState>>,
    outcome: String,
    note: Option<String>,
) -> ShellResult<VelocityView> {
    let outcome = CheckInOutcome::from_str(&outcome)
        .ok_or_else(|| ShellError::Invalid("unknown check-in outcome".into()))?;
    let today = today_local();
    state.with_identity(|identity| {
        let e = Engine::new(&state.repos, Some(identity));
        e.check_in(&today, outcome, note.as_deref())
            .map(|_| ())
            .map_err(ShellError::from)
    })?;
    velocity_inner(&state.repos)
}

#[derive(Serialize, Clone)]
pub struct VelocityView {
    pub milestones_remaining: i64,
    pub days_remaining: i64,
    pub target_per_day: f64,
    pub completion_ratio: f64,
    pub estimate_adjustment: f64,
}

fn velocity_inner(repos: &Repos) -> ShellResult<VelocityView> {
    let today = today_local();
    match wl_core::engine::velocity::compute(repos, &today)? {
        Some(v) => Ok(VelocityView {
            milestones_remaining: v.milestones_remaining,
            days_remaining: v.days_remaining,
            target_per_day: v.target_per_day,
            completion_ratio: v.completion_ratio,
            estimate_adjustment: v.estimate_adjustment,
        }),
        None => Ok(VelocityView {
            milestones_remaining: 0,
            days_remaining: 0,
            target_per_day: 0.0,
            completion_ratio: 1.0,
            estimate_adjustment: 1.0,
        }),
    }
}

#[tauri::command]
pub(crate) async fn velocity(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<VelocityView> {
    velocity_inner(&state.repos)
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) async fn settings_get(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<AppSettings> {
    Ok(state.repos.settings()?)
}

#[tauri::command]
pub(crate) async fn settings_save(
    app: tauri::AppHandle,
    state: State<'_, std::sync::Arc<AppState>>,
    settings: AppSettings,
) -> ShellResult<()> {
    // SSRF guard: the relay URL reaches format!("{base}{path}") with a
    // bearer token attached — refuse anything but a bare http(s) base
    // (no credentials/query/fragment, no link-local metadata hosts).
    if let Some(url) = settings.relay_url.as_deref() {
        if !url.trim().is_empty() {
            wl_core::net::validate_relay_url(url).map_err(ShellError::Invalid)?;
        }
    }
    let hotkey_changed = state
        .repos
        .settings()
        .map(|cur| cur.hotkey != settings.hotkey)
        .unwrap_or(true);
    state.with_identity_opt(|identity| {
        state
            .repos
            .save_settings(&settings, identity)
            .map_err(ShellError::from)
    })?;
    if hotkey_changed {
        crate::hotkey::register_summon_hotkey(&app, &settings.hotkey);
    }
    // Keep the live session coherent: a changed relay URL invalidates
    // any cached token (it belongs to a different server/account view).
    let mut url = state.relay_url.lock().unwrap();
    if *url != settings.relay_url {
        *url = settings.relay_url.clone();
        *state.relay_token.lock().unwrap() = None;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Sync (US-4)
// ---------------------------------------------------------------------------

struct ReqwestTransport {
    base: String,
    token: String,
}

/// One process-wide HTTP client: connection pooling across sync/handshake
/// calls, and no per-RPC TLS-stack rebuild (near-0-CPU on use; no idle
/// cost — construction is lazy on first sync).
fn shared_client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new).clone()
}

impl wl_sync::sync::Transport for ReqwestTransport {
    fn post(&self, path: &str, body: &serde_json::Value) -> Result<String, String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        rt.block_on(async {
            let resp = shared_client()
                .post(format!("{}{path}", self.base))
                .header("authorization", format!("Bearer {}", self.token))
                .json(body)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            let text = resp.text().await.map_err(|e| e.to_string())?;
            if !status.is_success() {
                return Err(format!("HTTP {status}: {text}"));
            }
            Ok(text)
        })
    }
}

#[derive(serde::Serialize)]
pub struct RelayAuthView {
    pub account_id: String,
    pub expires_at: i64,
}

/// Runs the challenge → sign → verify handshake and caches the bearer
/// token in memory (never persisted: sessions are short-lived by design).
/// Called automatically by `sync_now`; exposed for explicit re-auth and
/// the settings screen's connection test.
///
/// Body runs on a `spawn_blocking` thread: the sync stack drives nested
/// runtimes via block_on, which panics on async-runtime threads.
#[tauri::command]
pub(crate) async fn relay_authenticate(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<RelayAuthView> {
    let shared: std::sync::Arc<AppState> = (*state).clone();
    tokio::task::spawn_blocking(move || {
        let base = shared
            .relay_url
            .lock()
            .unwrap()
            .clone()
            .ok_or(ShellError::NoRelay)?;
        let base = wl_core::net::validate_relay_url(&base).map_err(ShellError::Invalid)?;
        let session = shared.with_identity(|identity| relay::handshake(&base, identity))?;
        let account_id = shared.with_identity(|id| Ok(id.account_id_hex()))?;
        *shared.relay_token.lock().unwrap() = Some(session.token.clone());
        Ok(RelayAuthView {
            account_id,
            expires_at: session.expires_at,
        })
    })
    .await
    .map_err(|e| ShellError::Relay(format!("auth task: {e}")))?
}

/// Ensures a cached bearer token, handshaking first when absent.
fn ensure_token(state: &AppState, base: &str) -> ShellResult<String> {
    if let Some(t) = state.relay_token.lock().unwrap().clone() {
        return Ok(t);
    }
    let token = state
        .with_identity(|identity| relay::handshake(base, identity).map(|s| s.token.clone()))?;
    *state.relay_token.lock().unwrap() = Some(token.clone());
    Ok(token)
}

fn run_cycle(
    state: &AppState,
    base: &str,
    token: &str,
) -> Result<wl_sync::sync::SyncStats, wl_sync::sync::SyncError> {
    state
        .with_identity(|identity| {
            let transport = ReqwestTransport {
                base: base.to_string(),
                token: token.to_string(),
            };
            wl_sync::sync::sync_cycle(&state.repos, identity, &transport, 200)
                .map_err(ShellError::Sync)
        })
        .map_err(|e| match e {
            ShellError::Sync(inner) => inner,
            other => wl_sync::sync::SyncError::Transport(format!("local: {other}")),
        })
}

#[tauri::command]
pub(crate) async fn sync_now(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<SyncStatsView> {
    // Same spawn_blocking discipline as relay_authenticate: sync I/O
    // below drives nested runtimes via block_on.
    let shared: std::sync::Arc<AppState> = (*state).clone();
    tokio::task::spawn_blocking(move || {
        let base = shared
            .relay_url
            .lock()
            .unwrap()
            .clone()
            .ok_or(ShellError::NoRelay)?;
        let base = wl_core::net::validate_relay_url(&base).map_err(ShellError::Invalid)?;
        let token = ensure_token(&shared, &base)?;
        let stats = match run_cycle(&shared, &base, &token) {
            // Session expired server-side (1h TTL) or relay restarted and
            // lost its session table: re-handshake exactly once, then retry.
            Err(wl_sync::sync::SyncError::Transport(msg)) if msg.starts_with("HTTP 401") => {
                *shared.relay_token.lock().unwrap() = None;
                let fresh = ensure_token(&shared, &base)?;
                run_cycle(&shared, &base, &fresh)?
            }
            other => other?,
        };
        // NOTE: the UI drives its HUD pill from this return value
        // (no event bridge needed while sync is on-demand only; revisit
        // if background auto-sync ever lands).
        let pending = shared
            .repos
            .pending_outbox(200)
            .map(|v| v.len())
            .unwrap_or(0);
        Ok(SyncStatsView {
            pushed: stats.pushed,
            pulled: stats.pulled,
            applied: stats.applied,
            pending,
        })
    })
    .await
    .map_err(|e| ShellError::Relay(format!("sync task: {e}")))?
}

#[derive(Serialize)]
pub struct SyncStatsView {
    pub pushed: usize,
    pub pulled: usize,
    pub applied: usize,
    /// Outbox ops still awaiting push (0 right after a clean cycle).
    pub pending: usize,
}

// ---------------------------------------------------------------------------
// AI tiers (BYOK — keys injected per-call from the vault, never stored
// in SQLite, never logged)
// ---------------------------------------------------------------------------

/// Reads the provider key from the vault (keys never cross the
/// command boundary from the UI anymore — Item 2).
fn vault_api_key(state: &AppState, provider: &str) -> ShellResult<Zeroizing<String>> {
    state
        .vault
        .get_api_key(provider)
        .map_err(|e| ShellError::Vault(e.to_string()))?
        .ok_or_else(|| ShellError::NoApiKey(provider.to_string()))
}

#[tauri::command]
pub(crate) async fn master_plan(
    state: State<'_, std::sync::Arc<AppState>>,
    provider: String,
    model: String,
    goal_title: String,
    goal_description: Option<String>,
    target_date: Option<String>,
    context: String,
) -> ShellResult<String> {
    let api_key = vault_api_key(&state, &provider)?;
    // Move the Zeroizing wrapper straight into the adapter: no
    // intermediate plain-String clone (the old `.to_string()` left a
    // non-zeroized copy on the heap next to the wiped original).
    let adapter = match provider.as_str() {
        "anthropic" => ProviderAdapter::Anthropic { api_key, model },
        _ => ProviderAdapter::OpenAiCompat {
            base_url: base_for(&provider),
            api_key,
            model,
        },
    };
    let (goal_id, _plan) = state.with_identity_opt(|identity| {
        AiDispatcher::master_plan(
            &adapter,
            &goal_title,
            goal_description.as_deref(),
            target_date.as_deref(),
            &context,
            http_execute,
            &state.repos,
            identity,
        )
        .map_err(ShellError::from)
    })?;
    Ok(goal_id)
}

fn base_for(provider: &str) -> String {
    match provider {
        "openrouter" => "https://openrouter.ai/api/v1".into(),
        "gemini-compat" => "https://generativelanguage.googleapis.com/v1beta/openai".into(),
        _ => "https://api.openai.com/v1".into(),
    }
}

fn http_execute(
    url: &str,
    headers: &[(String, String)],
    body: &serde_json::Value,
) -> Result<String, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        let mut req = shared_client().post(url).json(body);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        resp.text().await.map_err(|e| e.to_string())
    })
}

#[tauri::command]
pub(crate) async fn morning_briefing(
    state: State<'_, std::sync::Arc<AppState>>,
    provider: String,
    model: String,
    constraints: String,
) -> ShellResult<Vec<String>> {
    let api_key = vault_api_key(&state, &provider)?;
    // Same move-not-clone discipline as master_plan (see there).
    let adapter = match provider.as_str() {
        "anthropic" => ProviderAdapter::Anthropic { api_key, model },
        _ => ProviderAdapter::OpenAiCompat {
            base_url: base_for(&provider),
            api_key,
            model,
        },
    };
    let today = today_local();
    let velocity_json = serde_json::to_string(&velocity_inner(&state.repos)?).unwrap_or_default();
    let brief = state.with_identity_opt(|identity| {
        AiDispatcher::morning_briefing(
            &adapter,
            &today,
            &constraints,
            &velocity_json,
            http_execute,
            &state.repos,
            identity,
        )
        .map_err(ShellError::from)
    })?;
    Ok(brief.directives.into_iter().map(|d| d.title).collect())
}

// ---------------------------------------------------------------------------
// Window controls (PRD §2.2)
// ---------------------------------------------------------------------------

#[tauri::command]
pub(crate) async fn set_always_on_top(app: tauri::AppHandle, pinned: bool) -> ShellResult<()> {
    if let Some(win) = app.get_webview_window("main") {
        win.set_always_on_top(pinned)
            .map_err(|e| ShellError::Io(e.to_string()))?;
    }
    Ok(())
}

#[tauri::command]
pub(crate) async fn toggle_window_visibility(app: tauri::AppHandle) -> ShellResult<()> {
    crate::hotkey::toggle_window_visibility(&app);
    Ok(())
}

use tauri::Manager;
