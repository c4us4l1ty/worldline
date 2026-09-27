//! Tauri command surface — thin bridge: Dioxus UI → shell → wl-core.
//! Recovery display and key entry cross IPC; secrets are never persisted
//! in SQLite or temporary DOM dataset attributes.
//!
//! # Argument naming is part of the contract
//!
//! Every command below carries `rename_all = "snake_case"`, and that is
//! load-bearing rather than stylistic. `tauri-macros` defaults to
//! `ArgumentCase::Camel` (tauri-macros 2.6.3, `command/wrapper.rs:51`),
//! so a bare `#[tauri::command]` makes the shell look for `targetDate`
//! while the UI — which serialises plain serde structs, no
//! `rename_all` — sends `target_date`. The runtime resolves arguments
//! with a strict `payload.get(key)` and no default
//! (`tauri/src/ipc/command.rs:100-103`), so the mismatch is a hard
//! `command create_goal missing required key targetDate`, not a
//! silently-defaulted field.
//!
//! Snake_case is the convention the rest of the repo already speaks: the
//! Rust params, the browser mock in `ui/public/invoke-shim.js`, and the
//! `AGENTS.md` rule. Keeping it means a new multi-word parameter cannot
//! break the app by existing.
//! Single-word parameters are unaffected either way, so this changes
//! nothing that already worked. `src/ipc_tests.rs` drives the real
//! `generate_handler!` over a mock runtime to keep it that way.

use std::sync::Arc;

use serde::Serialize;
use tauri::State;
use zeroize::Zeroizing;

use wl_core::ai::catalog::ModelInfo;
use wl_core::ai::dispatch::{self, AiDispatcher, ProviderAdapter, PlanPreview};
use wl_core::crypto::identity::Identity;
use wl_core::domain::*;
use wl_core::engine::{calibration, Engine, EngineOutcome};
use wl_core::poison::LockRecover;
use wl_core::store::repo::Repos;

use crate::app_state::AppState;
use crate::error::{ShellError, ShellResult};
use crate::relay;

// ---------------------------------------------------------------------------
// Identity (US-1)
// ---------------------------------------------------------------------------

/// Generates a fresh 12-word identity. Returns the mnemonic ONCE for
/// onboarding display; it is persisted ONLY in the Stronghold vault
/// (encrypted with a device-local key), never in SQLite.
#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn identity_generate(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<GeneratedIdentity> {
    let mut current = state.identity.lock_recover();
    if current.is_some() || state.repos.identity()?.is_some() || state.vault.has_mnemonic() {
        return Err(ShellError::Invalid(
            "identity already exists; unlock or restore it".into(),
        ));
    }
    let identity = Identity::generate().map_err(|_| ShellError::BadMnemonic)?;
    let phrase = identity.phrase().to_string();
    let account_id = identity.account_id_hex();
    // Backup challenge positions: 3 distinct random indices into the
    // 12 words (Item 6 — no longer deterministic).
    let mut rng = rand::thread_rng();
    let verify_indices: Vec<usize> = rand::seq::index::sample(&mut rng, 12, 3).into_vec();
    persist_identity(&state, &mut current, identity, false, &verify_indices)?;
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
#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn identity_verify_backup(
    state: State<'_, std::sync::Arc<AppState>>,
    indices: Vec<usize>,
    words: Vec<String>,
) -> ShellResult<bool> {
    let guard = state.identity.lock_recover();
    let Some(id) = guard.as_ref() else {
        return Err(ShellError::Locked);
    };
    // Challenge positions come from persisted identity_config — never
    // from the caller — so a hostile UI payload can't pick its own.
    let cfg = state.repos.identity()?.ok_or(ShellError::Locked)?;
    if cfg.public_key != id.account_id_hex() {
        return Err(ShellError::Invalid(
            "stored identity does not match the vault".into(),
        ));
    }
    let stored = cfg.verify_indices;
    if !valid_backup_challenge(&stored) || indices != stored {
        return Ok(false);
    }
    let ok = id.verify_backup_words(&indices, &words);
    if ok {
        state.repos.set_mnemonic_verified(true)?;
    }
    Ok(ok)
}

fn valid_backup_challenge(indices: &[usize]) -> bool {
    indices.len() == 3
        && indices.iter().all(|index| *index < 12)
        && indices[0] != indices[1]
        && indices[0] != indices[2]
        && indices[1] != indices[2]
}

/// Restores an identity from a 12-word mnemonic (account recovery).
#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn identity_restore(
    state: State<'_, std::sync::Arc<AppState>>,
    phrase: String,
) -> ShellResult<String> {
    let phrase = Zeroizing::new(phrase);
    let mut current = state.identity.lock_recover();
    let identity = Identity::from_phrase(&phrase).map_err(|_| ShellError::BadMnemonic)?;
    let account_id = identity.account_id_hex();
    // Fresh challenge positions for the re-verified backup check.
    let mut rng = rand::thread_rng();
    let verify_indices: Vec<usize> = rand::seq::index::sample(&mut rng, 12, 3).into_vec();
    if state
        .repos
        .identity()?
        .is_some_and(|cfg| cfg.public_key != account_id)
    {
        return Err(ShellError::Invalid(
            "cannot replace this installation's identity".into(),
        ));
    }
    if state.vault.has_mnemonic() {
        let existing = state
            .vault
            .get_mnemonic()
            .map_err(|e| ShellError::Vault(e.to_string()))?;
        let existing = Identity::from_phrase(&existing).map_err(|_| ShellError::BadMnemonic)?;
        if existing.account_id_hex() != account_id {
            return Err(ShellError::Invalid(
                "cannot replace this installation's identity".into(),
            ));
        }
    }
    persist_identity(&state, &mut current, identity, true, &verify_indices)?;
    Ok(account_id)
}

fn persist_identity(
    state: &AppState,
    current: &mut Option<Arc<Identity>>,
    identity: Identity,
    verified: bool,
    verify_indices: &[usize],
) -> ShellResult<()> {
    // Vault first: persistence failure must not publish a new public or
    // process-resident identity without its recoverable secret.
    state
        .vault
        .save_mnemonic(identity.phrase())
        .map_err(|e| ShellError::Vault(e.to_string()))?;
    state
        .repos
        .insert_identity(&identity, verified, verify_indices)?;
    *current = Some(Arc::new(identity));
    Ok(())
}

/// Whether an identity exists (onboarding gate).
#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn identity_status(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<IdentityStatus> {
    let has = state.repos.identity()?.is_some();
    let unlocked = state.identity.lock_recover().is_some();
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
#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn identity_unlock(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<String> {
    let mut current = state.identity.lock_recover();
    let phrase = state
        .vault
        .get_mnemonic()
        .map_err(|e| ShellError::Vault(e.to_string()))?;
    let identity = Identity::from_phrase(phrase.as_str()).map_err(|_| ShellError::BadMnemonic)?;
    let account_id = identity.account_id_hex();
    // A vault/public-state mismatch must not silently switch accounts.
    // Missing legacy challenges remain unverified until recovery proves
    // possession of the full phrase through identity_restore.
    let (verified, indices) = match state.repos.identity()? {
        Some(cfg) if cfg.public_key == account_id => {
            let verified = cfg.bip39_mnemonic_verified;
            (verified, cfg.verify_indices)
        }
        Some(_) => {
            return Err(ShellError::Invalid(
                "stored identity does not match the vault".into(),
            ))
        }
        None => (false, Vec::new()),
    };
    state.repos.insert_identity(&identity, verified, &indices)?;
    *current = Some(Arc::new(identity));
    Ok(account_id)
}

// ---------------------------------------------------------------------------
// BYOK key management (Item 2) — secrets enter via these commands and
// never leave the vault except into an outbound HTTPS request body.
// ---------------------------------------------------------------------------

/// Stores (or replaces) a provider API key in the vault.
#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn set_api_key(
    state: State<'_, std::sync::Arc<AppState>>,
    provider: String,
    key: String,
) -> ShellResult<bool> {
    let key = Zeroizing::new(key);
    // Real provider keys are ~100–200 chars; anything past 8 KiB is a
    // paste accident that would bloat the encrypted vault snapshot.
    if key.trim().is_empty() {
        return Err(ShellError::Invalid("empty API key".into()));
    }
    if key.trim().chars().count() > 8192 {
        return Err(ShellError::Invalid(
            "API key exceeds 8192 characters".into(),
        ));
    }
    state
        .vault
        .save_api_key(&provider, key.trim())
        .map_err(|e| ShellError::Vault(e.to_string()))?;
    Ok(true)
}

/// Whether a key is stored for the provider (presence only — the key
/// itself is never readable through commands).
#[tauri::command(rename_all = "snake_case")]
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
#[tauri::command(rename_all = "snake_case")]
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

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn create_goal(
    state: State<'_, std::sync::Arc<AppState>>,
    title: String,
    description: Option<String>,
    target_date: Option<String>,
    complexity: i64,
) -> ShellResult<GoalJson> {
    // Identity is optional here: logged-out goal drafts still work, but
    // only an unlocked vault write-throughs to the outbox.
    let g = state.with_identity_opt(|identity| {
        create_seeded_goal(
            &state.repos,
            &title,
            description.as_deref(),
            target_date.as_deref(),
            complexity,
            identity,
        )
        .map_err(ShellError::from)
    })?;
    Ok(goal_json(&g, &state.repos))
}

/// Creates a goal and leaves it runnable, superseding any goal it
/// replaces.
///
/// The `archive_other_active_goals` call is the same one
/// `wl_core::ai::dispatch::persist_plan` makes. Without it a second
/// manual goal left two rows `active`, and `Repos::active_goal()` —
/// `ORDER BY hlc_timestamp LIMIT 1` — bound velocity and recovery to
/// the OLDER, superseded goal. Runs after the new goal exists so
/// `keep` can never archive the goal that was just created.
fn create_seeded_goal(
    repos: &Repos,
    title: &str,
    description: Option<&str>,
    target_date: Option<&str>,
    complexity: i64,
    identity: Option<&Identity>,
) -> Result<Goal, wl_core::store::StoreError> {
    let goal = repos.create_goal(title, description, target_date, complexity, identity)?;
    repos.archive_other_active_goals(&goal.id, identity)?;
    // B-002 (option b): a manual goal must never be a dead end. Seed
    // one milestone + one directive from the goal title so the canvas
    // has a runnable directive with NO API key (the AI restructures
    // the plan once a key exists). Authoring UI for further
    // milestones/directives is Phase-2 MVP-1; the manual shell
    // commands already exist for it.
    seed_first_steps(repos, &goal, identity)?;
    Ok(goal)
}

/// Seeds the manual-goal starter set: milestone "First steps" + one
/// 25-minute directive titled from the goal, scheduled today so
/// `Engine::current` activates it on the next canvas load. 25 minutes
/// stays under the progressive threshold — no phases to author.
fn seed_first_steps(
    repos: &Repos,
    goal: &Goal,
    identity: Option<&Identity>,
) -> Result<(), wl_core::store::StoreError> {
    let m = repos.create_milestone(&goal.id, "First steps", None, 0, identity)?;
    repos.create_directive(
        &m.id,
        &goal.title,
        goal.description.as_deref(),
        25,
        1,
        &today_local(),
        None,
        &[],
        identity,
    )?;
    Ok(())
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

/// One row of the Entropy Log, mirrored from `wl_core`'s `LedgerRow`.
///
/// A mirror rather than a derived `Serialize` on the wl-core type: the
/// shell's own domain types are not part of the JSON contract, so the wire
/// format is declared here, at the edge that owns it.
#[derive(Serialize, Clone)]
pub struct TaskView {
    pub directive_id: String,
    pub goal_id: String,
    pub goal_title: String,
    pub milestone_title: String,
    pub title: String,
    pub instruction: Option<String>,
    pub estimated_minutes: i64,
    /// `queued` | `active` | `completed` | `blocked` | `skipped`
    pub state: String,
    /// The goal's *Estimated Complexity* word, resolved in the core so the
    /// UI cannot invent a second table of them.
    pub complexity_label: Option<String>,
    /// The title of the unfinished task this one is waiting on.
    pub blocked_by: Option<String>,
    /// `(step, total)`, already filtered by the store so a corrupt
    /// replicated `progressive_step` cannot render "Phase 9 of 4".
    pub phase: Option<(i64, i64)>,
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn mark_task_done(
    state: State<'_, std::sync::Arc<AppState>>,
    directive_id: String,
) -> ShellResult<DirectiveView> {
    let today = today_local();
    // `with_identity_opt`, not the strict variant: a task is done whether
    // or not the vault happens to be open, exactly like a manual goal. The
    // identity only decides whether the change replicates — refusing the
    // write would mean a locked vault made the ledger unusable, which is
    // the one thing a ledger must never be.
    let out = state.with_identity_opt(|identity| {
        let e = Engine::new(&state.repos, identity);
        e.mark_complete(&today, &directive_id)?;
        // Same two-step shape as `complete_directive`: mark, then re-read
        // the canvas. The ledger is where a task is resolved, and the
        // canvas is what shows the next one — so a tick that resolved the
        // ACTIVE directive has to advance it here, or the canvas is left
        // showing a task that is already done.
        e.current(&today).map_err(ShellError::from)
    })?;
    Ok(outcome_view(&out, &state.repos))
}

/// How many tasks the ledger asks for.
///
/// The ledger is a glance, not an archive: a hundred rows is several
/// screens of history on a bad stretch and still one cheap join, while
/// several hundred would be a page the control panel has no UI for.
/// Bounded so a long-lived install cannot make every panel open an
/// unbounded three-table join; `wl-core` clamps to its own ceiling as
/// well, so the two limits cannot drift into a footgun.
const TASK_LEDGER_LIMIT: usize = 100;

/// Every task, open work first, joined up to its goal.
#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn task_ledger(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<Vec<TaskView>> {
    let rows = state
        .repos
        .task_ledger(TASK_LEDGER_LIMIT)
        .map_err(ShellError::from)?;
    Ok(rows.into_iter().map(task_view).collect())
}

fn task_view(row: wl_core::store::repo::LedgerRow) -> TaskView {
    // Every derived field is read before any field is moved out. Two of
    // these are methods on the row rather than raw field reads precisely so
    // the order stops mattering.
    let complexity_label = row.complexity_label().map(str::to_string);
    let phase = row.phase_view();
    TaskView {
        directive_id: row.directive_id,
        goal_id: row.goal_id,
        goal_title: row.goal_title,
        milestone_title: row.milestone_title,
        title: row.title,
        instruction: row.execution_context,
        estimated_minutes: row.estimated_minutes,
        state: row.state.as_str().into(),
        complexity_label,
        blocked_by: row.blocked_by,
        phase,
    }
}

/// Every active goal with its milestone progress, newest first.
#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn list_goals(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<Vec<GoalJson>> {
    let rows = state
        .repos
        .active_goals_with_progress()
        .map_err(ShellError::from)?;
    Ok(rows
        .into_iter()
        .map(|g| GoalJson {
            id: g.id,
            title: g.title,
            target_date: g.target_date,
            milestone_done: g.milestones_done,
            milestone_total: g.milestones_total,
        })
        .collect())
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
        .lock_conn()
        .query_row(
            "SELECT title FROM milestones WHERE id = ?1",
            [milestone_id],
            |r| r.get::<_, String>(0),
        )
        .ok()
}

#[tauri::command(rename_all = "snake_case")]
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


// ---------------------------------------------------------------------------
// Check-in & velocity (PRD §5.4)
// ---------------------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
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

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn velocity(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<VelocityView> {
    velocity_inner(&state.repos)
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn settings_get(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<AppSettings> {
    Ok(sanitize_settings(state.repos.settings()?))
}

/// Drops an `ai_provider` this build does not support.
///
/// `save_settings` rejects any provider outside `KNOWN_PROVIDERS`, and
/// that check is the reason this has to exist. A provider removed from
/// the allow-list stays in `app_settings` on every install that had it
/// selected — and since `app_settings` is a replicated CRDT table, a
/// peer can reintroduce one at any time — and because `settings_save`
/// writes the whole struct, such an install could then not save *any*
/// setting, including a theme toggle, failing with "unknown AI
/// provider". Sanitizing on read makes the removal a non-event.
///
/// Tier model ids are left alone: they are only ever sent back to a
/// provider, and clearing them here would silently discard a choice the
/// user can still make sense of once they re-pick a provider.
fn sanitize_settings(mut s: AppSettings) -> AppSettings {
    if let Some(p) = s.ai_provider.as_deref() {
        if !KNOWN_PROVIDERS.contains(&p) {
            s.ai_provider = None;
        }
    }
    s
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn settings_save(
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
    let previous_relay_url = state.repos.settings()?.relay_url;
    state.with_identity_opt(|identity| {
        state
            .repos
            .save_settings(&settings, identity)
            .map_err(ShellError::from)
    })?;
    // Keep the live session coherent: a changed relay URL invalidates
    // any cached token (it belongs to a different server/account view).
    // `RelaySession::token_for` already refuses a token from another
    // base, so this only drops the dead bearer secret early.
    if previous_relay_url != settings.relay_url {
        *state.relay_token.lock_recover() = None;
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

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| e.to_string())
}

/// Process-wide HTTP client: connection pools (TCP/TLS sessions) are
/// reused across sync/AI calls instead of rebuilt per request. `Client`
/// is an `Arc` internally, so `&'static` access is race-free.
pub(crate) fn shared_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::LazyLock<reqwest::Client> =
        std::sync::LazyLock::new(|| http_client().expect("client builds from fixed valid config"));
    &CLIENT
}

/// Process-wide Tokio runtime for the blocking HTTP layer.
///
/// The sync transport and the AI execute closures are synchronous, so
/// each of them drives a future to completion with `block_on` from a
/// `spawn_blocking` thread. That future has to be driven by ONE runtime
/// for the life of the process, not one per request: hyper spawns every
/// connection's driver task onto whichever runtime is current when the
/// request is made (`hyper-util` `TokioExecutor` → `tokio::spawn`),
/// and dropping that runtime kills the task — which kills the pooled
/// connection the next request was going to reuse. A per-call runtime
/// therefore paid a fresh TCP+TLS handshake for every relay POST and
/// every AI call (a 200-batch `sync_now` meant 200 handshakes) and left
/// the pool's idle sweeper dead for the life of the process.
///
/// Sharing the runtime does not weaken the discipline: `block_on` still
/// panics when called from inside a runtime, so every caller must stay
/// on `spawn_blocking` (see AGENTS.md). Concurrent callers are fine —
/// the current-thread scheduler hands the driver to one of them and
/// polls the others' futures itself — which is what keeps two
/// concurrent commands from serialising on the network.
pub(crate) fn shared_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::LazyLock<tokio::runtime::Runtime> = std::sync::LazyLock::new(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime builds from fixed valid config")
    });
    &RUNTIME
}

impl wl_sync::sync::Transport for ReqwestTransport {
    fn post(&self, path: &str, body: &serde_json::Value) -> Result<String, String> {
        shared_runtime().block_on(async {
            let resp = shared_client()
                .post(format!("{}{path}", self.base))
                .header("authorization", format!("Bearer {}", self.token))
                .json(body)
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let status = resp.status();
            if !status.is_success() {
                return Err(format!("HTTP {status}"));
            }
            // SYNC-4: the reader's ceiling is the protocol's, not a
            // hand-picked 16 MiB. It must be at least the relay's
            // emission budget or a legitimate full pull would be
            // truncated client-side and look like data loss; being
            // exactly equal means the relay's own budget is what binds.
            relay::read_body(resp, wl_protocol::MAX_PULL_BYTES).await
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
/// Body runs on a `spawn_blocking` thread: the sync stack drives the
/// shared runtime via block_on, which panics on async-runtime threads.
#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn relay_authenticate(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<RelayAuthView> {
    let shared: std::sync::Arc<AppState> = (*state).clone();
    tokio::task::spawn_blocking(move || {
        let base = shared.relay_base()?;
        // Taken as a handle, not a guard: the handshake can take 120 s
        // and every other identity-gated command has to keep running
        // while it does.
        let identity = shared.identity_handle().ok_or(ShellError::Locked)?;
        let session = relay::handshake(&base, &identity, shared_client())?;
        let account_id = identity.account_id_hex();
        *shared.relay_token.lock_recover() = Some(crate::app_state::RelaySession {
            base,
            account_id: account_id.clone(),
            token: session.token,
        });
        Ok(RelayAuthView {
            account_id,
            expires_at: session.expires_at,
        })
    })
    .await
    .map_err(|e| ShellError::Relay(format!("auth task: {e}")))?
}

/// Ensures a cached bearer token, handshaking first when absent.
fn ensure_token(state: &AppState, base: &str, identity: &Identity) -> ShellResult<String> {
    let account_id = identity.account_id_hex();
    if let Some(token) = state
        .relay_token
        .lock_recover()
        .as_ref()
        .and_then(|session| session.token_for(base, &account_id))
    {
        return Ok(token.to_string());
    }
    let token = relay::handshake(base, identity, shared_client())?.token;
    *state.relay_token.lock_recover() = Some(crate::app_state::RelaySession {
        base: base.to_string(),
        account_id,
        token: token.clone(),
    });
    Ok(token)
}

fn run_cycle(
    state: &AppState,
    identity: &Identity,
    base: &str,
    token: &str,
) -> Result<wl_sync::sync::SyncStats, wl_sync::sync::SyncError> {
    let transport = ReqwestTransport {
        base: base.to_string(),
        token: token.to_string(),
    };
    wl_sync::sync::sync_cycle(&state.repos, identity, &transport, 200)
}

#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn sync_now(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<SyncStatsView> {
    // Same spawn_blocking discipline as relay_authenticate: sync I/O
    // below drives the shared runtime via block_on.
    let shared: std::sync::Arc<AppState> = (*state).clone();
    tokio::task::spawn_blocking(move || {
        let base = shared.relay_base()?;
        // A handle, not a guard: one cycle is up to 200 HTTP posts.
        let identity = shared.identity_handle().ok_or(ShellError::Locked)?;
        let token = ensure_token(&shared, &base, &identity)?;
        let stats = match run_cycle(&shared, &identity, &base, &token) {
            Err(wl_sync::sync::SyncError::Transport(msg)) if msg.starts_with("HTTP 401") => {
                *shared.relay_token.lock_recover() = None;
                let fresh = ensure_token(&shared, &base, &identity)?;
                run_cycle(&shared, &identity, &base, &fresh).map_err(ShellError::from)
            }
            other => other.map_err(ShellError::from),
        }?;
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
            quarantined: stats.quarantined,
            cursor: stats.cursor,
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
    /// MVP-4: poison/unknown-table ops watermarked-and-skipped this
    /// cycle — surfaced in telemetry so skips are never silent.
    pub quarantined: usize,
    /// MVP-4: pull position after this cycle (`hlc,op_id` composite).
    pub cursor: String,
}

// ---------------------------------------------------------------------------
// Difficulty calibration (the *Estimated Complexity* estimator)
// ---------------------------------------------------------------------------

/// One difficulty bucket, with the evidence behind it.
#[derive(Serialize, Clone)]
pub struct CalibrationView {
    /// 1–5, as the user rated it.
    pub complexity: i64,
    /// `light` … `deep`, resolved in the core.
    pub label: String,
    /// Posterior mean, 0–100. Meaningless without `evidence`, which is why
    /// both are always returned together.
    pub percent: i64,
    /// `"4 of 9"` — the raw counts, deliberately. "44%" is not checkable
    /// against the ledger; "4 of 9" is.
    pub evidence: String,
    pub observations: i64,
}

/// The estimator's current state, one row per bucket the user has rated.
#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn calibration_view(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<Vec<CalibrationView>> {
    let buckets = calibration::calibration(&state.repos, &today_local())?;
    Ok(buckets
        .values()
        .filter_map(|b| {
            // A bucket with no label means the replicated rating was out of
            // range in a way the clamp could not hide, and a row with no
            // word is worse than no row.
            Some(CalibrationView {
                complexity: b.complexity,
                label: complexity_label(b.complexity)?.to_string(),
                percent: (b.posterior_mean() * 100.0).round() as i64,
                evidence: b.evidence(),
                observations: b.observations,
            })
        })
        .collect())
}

// ---------------------------------------------------------------------------
// AI tiers (BYOK — keys injected per-call from the vault, never stored
// in SQLite, never logged)
// ---------------------------------------------------------------------------

/// Reads the provider key from the vault (keys never cross the
/// command boundary from the UI anymore — Item 2). The allow-list is
/// exactly `wl_core::domain::KNOWN_PROVIDERS` (B-001 regression test
/// below pins them equal).
fn vault_api_key(state: &AppState, provider: &str) -> ShellResult<Zeroizing<String>> {
    if !KNOWN_PROVIDERS.contains(&provider) {
        return Err(ShellError::Invalid("unknown AI provider".into()));
    }
    state
        .vault
        .get_api_key(provider)
        .map_err(|e| ShellError::Vault(e.to_string()))?
        .ok_or_else(|| ShellError::NoApiKey(provider.to_string()))
}

/// Rejects a blank model id before any request is made.
///
/// There used to be a fallback here — the compose screen sent
/// `"flagship"` and the briefing sent `"haiku-class"` when nothing was
/// configured. Neither string is a real model on any provider, so the
/// request 400'd, the error was discarded, and the UI reported
/// "OFFLINE". A blank id is now a refusal with a pointer to Settings,
/// which is both honest and free.
fn require_model(provider: &str, model: &str) -> ShellResult<()> {
    if model.trim().is_empty() {
        return Err(ShellError::Invalid(format!(
            "no model selected — choose an architect model for {provider} in Settings"
        )));
    }
    Ok(())
}

/// How long one Tier-1 call may take before the shell gives up on it.
///
/// The old shape had NO bound of its own: the only ceiling anywhere on the
/// path was reqwest's 120 s client timeout, which is a number chosen for
/// "don't hang forever", not one a person can wait through. A two-minute
/// button with no stage text and no cancel is the shape of the bug this
/// whole command surface was rebuilt for.
///
/// 75 s is above a real plan request (a 4 096-token completion over a
/// consumer link is seconds) and below a wait a user reads as broken. The
/// bounds are compile-time rather than a test, in the same spirit as
/// `BOOT_TIMEOUT_MS` on the UI side.
// `Duration`'s comparison operators are not `const`, so the bounds are
// stated over the seconds the value is built from — which is also the
// thing the reader cares about.
const TIER1_TIMEOUT_SECS: u64 = 75;
const _: () = assert!(
    TIER1_TIMEOUT_SECS >= 30,
    "a cold start plus a real completion must fit inside the budget"
);
const _: () = assert!(
    TIER1_TIMEOUT_SECS <= 120,
    "and must not exceed the HTTP client timeout, or the transport error is what the user sees"
);
pub const TIER1_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(TIER1_TIMEOUT_SECS);

/// Whether the architect can be called at all, and what is missing if not.
///
/// A local, instant, **non-billable** check that runs before the call.
/// Every configuration mistake — no provider, no key, no model — used to be
/// discovered after a request had already left, or in the case of a missing
/// model, after a 2.2-second toast had come and gone. This turns the
/// common failures into an answer in milliseconds.
///
/// Returns the ready state rather than erroring, because the compose screen
/// wants to show "not configured" as a state, not an exception.
#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn ai_readiness(
    state: State<'_, std::sync::Arc<AppState>>,
) -> ShellResult<AiReadiness> {
    let settings = state.repos.settings()?;
    let provider = settings
        .ai_provider
        .clone()
        .filter(|p| KNOWN_PROVIDERS.contains(&p.as_str()))
        .unwrap_or_else(|| "openrouter".to_string());
    let model = settings.tier1_model.clone().unwrap_or_default();
    let has_key = state
        .vault
        .has_api_key(&provider)
        .map_err(|e| ShellError::Vault(e.to_string()))?;
    Ok(AiReadiness {
        provider,
        model_set: !model.trim().is_empty(),
        model,
        key_set: has_key,
    })
}

#[derive(Serialize, Clone)]
pub struct AiReadiness {
    pub provider: String,
    pub model: String,
    pub model_set: bool,
    pub key_set: bool,
}

impl AiReadiness {
    /// The first thing that is missing, phrased for the page rather than
    /// for a log.
    ///
    /// Ordered by what the user has to do first: a provider is implied by
    /// the default, a key is a paste, and a model is a long list to scroll.
    /// Both are reported so the page can disable the button and say why in
    /// one line rather than making the user press it to find out.
    pub fn missing(&self) -> Option<String> {
        if !self.model_set {
            return Some("Choose an architect model in Settings.".into());
        }
        if !self.key_set {
            return Some(format!("No API key stored for {}.", self.provider));
        }
        None
    }
}

#[tauri::command(rename_all = "snake_case")]
/// Tier 1, stage one: fetch a plan and return it **without writing
/// anything**.
///
/// `intent` is raw user text, NOT a title: the architect names the goal
/// from it. `complexity` is the compose screen's *Estimated Complexity*
/// rating, 1–5, and it goes into both the prompt and the returned preview
/// so `commit_plan` cannot lose it.
///
/// The wire keys are these parameter names verbatim, snake_case included —
/// see the module header for why `rename_all` is not optional.
pub(crate) async fn master_plan_preview(
    state: State<'_, std::sync::Arc<AppState>>,
    provider: String,
    model: String,
    intent: String,
    target_date: Option<String>,
    complexity: i64,
) -> ShellResult<PlanPreview> {
    require_model(&provider, &model)?;
    check_complexity("complexity", complexity).map_err(ShellError::Invalid)?;
    let shared: std::sync::Arc<AppState> = (*state).clone();
    let task = tokio::task::spawn_blocking(move || {
        let api_key = vault_api_key(&shared, &provider)?;
        // Move the Zeroizing wrapper straight into the adapter: no
        // intermediate plain-String clone (the old `.to_string()` left a
        // non-zeroized copy on the heap next to the wiped original).
        // Every approved provider speaks OpenAI-compatible chat
        // completions; only the base URL varies (B-001).
        let base_url =
            base_for(&provider).ok_or_else(|| ShellError::Invalid("unknown AI provider".into()))?;
        // `response_format` goes out only for models the catalog says
        // accept it. Unknown (no catalog entry) keeps the old behaviour,
        // so this can never make a working call fail — it only stops
        // sending a field that some models reject outright.
        let json_mode = wl_core::ai::catalog::json_mode_for(
            shared.cached_model(&provider, model.trim()).as_ref(),
        );
        let adapter = ProviderAdapter::OpenAiCompat {
            base_url,
            api_key,
            model: model.trim().to_string(),
            json_mode,
        };
        let err_provider = provider.clone();
        let execute = move |url: &str, h: &[(String, String)], b: &serde_json::Value| {
            http_execute(&err_provider, url, h, b)
        };
        // The estimator's read of the bucket the user just picked, as one
        // sentence for the prompt. This is the number's ONLY consumer, and
        // without it the calibration figure is a display with no effect.
        let record = calibration::prompt_line(
            &calibration::calibration(&shared.repos, &today_local())?,
            complexity,
        );
        AiDispatcher::master_plan_preview(
            &adapter,
            &intent,
            target_date.as_deref(),
            complexity,
            record.as_deref(),
            execute,
        )
        .map_err(ShellError::from)
    });
    // The deadline is applied HERE rather than inside the blocking task, so
    // the shell can stop waiting and answer even if the task is wedged in
    // something the HTTP timeout does not cover. The task is abandoned, not
    // cancelled — `spawn_blocking` cannot be cancelled — which is exactly
    // why the UI needs its own watchdog: this is a bound on the shell's
    // patience, not a guarantee about the thread.
    match tokio::time::timeout(TIER1_TIMEOUT, task).await {
        Ok(Ok(result)) => result,
        Ok(Err(e)) => Err(ShellError::Io(format!("architect task: {e}"))),
        Err(_) => Err(ShellError::Provider(format!(
            "the architect did not answer within {}s — the plan was not created",
            TIER1_TIMEOUT.as_secs()
        ))),
    }
}

#[tauri::command(rename_all = "snake_case")]
/// Tier 1, stage two: write the plan the user approved.
///
/// The `preview` is a `struct` parameter, so the UI has to wrap it as
/// `{"preview": {...}}` — see the module header. It is a parameter that
/// must NOT be `Option`: a missing `preview` here would silently commit
/// nothing and report success, and the user would be told their plan was
/// on the line while the canvas was empty.
pub(crate) async fn commit_plan(
    state: State<'_, std::sync::Arc<AppState>>,
    preview: PlanPreview,
    target_date: Option<String>,
    intent: Option<String>,
) -> ShellResult<CommitOutcome> {
    let shared: std::sync::Arc<AppState> = (*state).clone();
    // Cheap and local, so no spawn_blocking: this is SQLite work on an
    // in-memory plan.
    let out = shared.with_identity_opt(|identity| {
        dispatch::persist_plan(
            &shared.repos,
            intent
                .as_deref()
                .map(|i| (i, None, target_date.as_deref())),
            &preview,
            identity,
        )
        .map_err(ShellError::from)
    })?;
    Ok(CommitOutcome {
        goal_id: out.goal_id,
        warnings: out.warnings,
    })
}

#[derive(Serialize)]
pub struct CommitOutcome {
    pub goal_id: String,
    /// What repair changed, plus any edge that had to be dropped. Shown on
    /// the canvas as a receipt, because a plan the user approved and a row
    /// that was written are allowed to differ — and they should never
    /// differ silently.
    pub warnings: Vec<String>,
}

/// OpenAI-compatible base URL per approved provider id.
///
/// Now a thin delegation to [`wl_core::ai::catalog::base_for`], which is
/// where the table lives. It moved because the model catalog needs the
/// same base URL to build its `/models` endpoint, and a provider table
/// that exists in two places is a provider table that will drift.
fn base_for(provider: &str) -> Option<String> {
    wl_core::ai::catalog::base_for(provider)
}

/// Largest AI response we will read. A completion is a few KB; the
/// catalog is ~750 KB for OpenRouter's 458 models, so this has to clear
/// that with room to spare while still refusing a hostile multi-GB body.
/// Error *messages* are bounded separately, inside
/// `ai::catalog::provider_error_message`.
const MAX_AI_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// POSTs a chat completion. `provider` is carried only so a failure can
/// be phrased for the provider that produced it (Google's 403 means
/// something specific and actionable; OpenRouter's does not).
fn http_execute(
    provider: &str,
    url: &str,
    headers: &[(String, String)],
    body: &serde_json::Value,
) -> Result<String, String> {
    shared_runtime().block_on(async {
        let mut req = shared_client().post(url).json(body);
        for (k, v) in headers {
            req = req.header(k, v);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let text = read_ai_body(resp, MAX_AI_RESPONSE_BYTES).await?;
        if !(200..300).contains(&status) {
            // The body IS the diagnosis. A provider usually says exactly
            // what is wrong ("No endpoints found matching model
            // 'flagship'"); discarding it is why every failure used to
            // reach the UI as a bare "HTTP 400" and then as "OFFLINE".
            return Err(wl_core::ai::catalog::provider_error_message(
                provider, status, &text,
            ));
        }
        Ok(text)
    })
}

/// GETs a provider's model catalog. `auth` is the raw key, sent as a
/// bearer; two of the three providers 401 without one.
fn http_get_catalog(provider: &str, url: &str, auth: &str) -> Result<String, String> {
    shared_runtime().block_on(async {
        let resp = shared_client()
            .get(url)
            .header("Authorization", format!("Bearer {auth}"))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let status = resp.status().as_u16();
        let text = read_ai_body(resp, MAX_AI_RESPONSE_BYTES).await?;
        if !(200..300).contains(&status) {
            return Err(wl_core::ai::catalog::provider_error_message(
                provider, status, &text,
            ));
        }
        Ok(text)
    })
}

/// Reads a response body under a hard ceiling, enforced per chunk as
/// well as by `content_length` so a chunked body cannot slip past.
async fn read_ai_body(mut resp: reqwest::Response, max: usize) -> Result<String, String> {
    if resp.content_length().is_some_and(|len| len > max as u64) {
        return Err("AI response body too large".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if chunk.len() > max - bytes.len() {
            return Err("AI response body too large".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct ModelListView {
    pub provider: String,
    pub models: Vec<ModelInfo>,
    /// Display names for the picker's "Recommended" group, resolved
    /// against this catalog. Empty means the group is omitted — it is
    /// never rendered as an empty section, and nothing is auto-selected
    /// from it.
    pub recommended: Vec<String>,
    /// `true` when this response came from the in-memory cache rather
    /// than the provider.
    pub cached: bool,
    /// The provider's own total, when it reports one and the cap bit.
    /// `None` means the list is complete.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated_from: Option<usize>,
}

/// Lists the models a provider currently serves, so the UI can offer real
/// ids instead of a text field where a typo costs an API call to find.
///
/// Key-gated for all three providers (OpenRouter's list happens to be
/// public, but one rule beats three, and a keyless install should not
/// pull a catalog it cannot use). Cached in memory for an hour;
/// `force` bypasses it, which is how a newly released model is picked up
/// without waiting for the TTL.
#[tauri::command(rename_all = "snake_case")]
pub(crate) async fn list_models(
    state: State<'_, std::sync::Arc<AppState>>,
    provider: String,
    force: Option<bool>,
) -> ShellResult<ModelListView> {
    if !KNOWN_PROVIDERS.contains(&provider.as_str()) {
        return Err(ShellError::Invalid("unknown AI provider".into()));
    }
    let force = force.unwrap_or(false);
    let shared: std::sync::Arc<AppState> = (*state).clone();
    if !force {
        if let Some((models, truncated_from)) = shared.catalog_fresh(&provider) {
            return Ok(model_list_view(provider, models, truncated_from, true));
        }
    }
    // Sync I/O drives the shared runtime: spawn_blocking or it panics.
    tokio::task::spawn_blocking(move || {
        let key = vault_api_key(&shared, &provider)?;
        let ep = wl_core::ai::catalog::models_endpoint(&provider)
            .ok_or_else(|| ShellError::Invalid("unknown AI provider".into()))?;
        let body =
            http_get_catalog(&provider, &ep.url, key.as_str()).map_err(ShellError::Provider)?;
        let models = wl_core::ai::catalog::parse_for(&provider, &body)
            .map_err(|e| ShellError::Provider(e.to_string()))?;
        if models.is_empty() {
            return Err(ShellError::Provider(
                "provider returned no models — check the key for this provider".into(),
            ));
        }
        let truncated_from =
            (models.len() >= wl_core::ai::catalog::MAX_MODELS).then_some(models.len());
        let view = model_list_view(provider.clone(), models.clone(), truncated_from, false);
        shared.store_catalog(&provider, models, truncated_from);
        Ok(view)
    })
    .await
    .map_err(|e| ShellError::Io(format!("list models task: {e}")))?
}

/// The one place a `ModelListView` is assembled, so the cached and the
/// freshly-fetched path cannot disagree. The truncation flag used to be
/// hardcoded to `None` on the cached path, so a provider with at least
/// `MAX_MODELS` models told the UI its list was complete on every visit
/// after the first.
fn model_list_view(
    provider: String,
    models: Vec<ModelInfo>,
    truncated_from: Option<usize>,
    cached: bool,
) -> ModelListView {
    ModelListView {
        recommended: recommended_ids(&provider, &models),
        provider,
        models,
        cached,
        truncated_from,
    }
}

/// Ids from the curated "Recommended" list that this catalog actually
/// contains. Pure ordering aid — it never gates a model.
fn recommended_ids(provider: &str, models: &[ModelInfo]) -> Vec<String> {
    wl_core::ai::catalog::recommended(provider, models)
        .into_iter()
        .map(|m| m.id.clone())
        .collect()
}

// There are no window-control commands. `set_always_on_top` used to live
// here, backing the Settings pin switch; both are gone (PRD delta 175), and
// so is the boot-time restore of the persisted value in `main.rs`. The
// `app_settings.always_on_top` column remains in SQLite and is simply
// never written or read — see the note in `wl-core`'s `save_settings`.
//

#[cfg(test)]
mod tests {
    use super::*;
    use wl_sync::sync::Transport;

    #[test]
    fn identity_snapshot_failure_preserves_memory_disk_and_public_state() {
        let dir = std::env::temp_dir().join(format!("wl-identity-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let state = AppState {
            repos: Repos::new(wl_core::store::open_in_memory().unwrap(), 1),
            identity: std::sync::Mutex::new(None),
            vault: crate::vault::Vault::open(&dir).unwrap(),
            relay_token: std::sync::Mutex::new(None),
            catalog: std::sync::Mutex::new(std::collections::HashMap::new()),
        };
        let mut current = None;
        let original = Identity::generate().unwrap();
        let account = original.account_id_hex();
        let phrase = Zeroizing::new(original.phrase().to_string());
        persist_identity(&state, &mut current, original, false, &[1, 4, 8]).unwrap();
        let before = state.repos.identity().unwrap().unwrap();
        // Preserve the durable snapshot, then make its path unwritable
        // regardless of process privileges (a directory cannot be a file).
        let snapshot = dir.join("vault.hold");
        let saved = dir.join("saved.hold");
        std::fs::rename(&snapshot, &saved).unwrap();
        std::fs::create_dir(&snapshot).unwrap();
        let replacement = Identity::generate().unwrap();
        assert!(matches!(
            persist_identity(&state, &mut current, replacement, true, &[0, 3, 7]),
            Err(ShellError::Vault(_))
        ));
        assert_eq!(current.as_ref().unwrap().account_id_hex(), account);
        assert_eq!(
            state.vault.get_mnemonic().unwrap().as_str(),
            phrase.as_str()
        );
        let after = state.repos.identity().unwrap().unwrap();
        assert_eq!(after.public_key, before.public_key);
        assert_eq!(
            after.bip39_mnemonic_verified,
            before.bip39_mnemonic_verified
        );
        assert_eq!(after.verify_indices, before.verify_indices);
        let rows: i64 = state
            .repos
            .conn
            .lock()
            .expect("test DB mutex")
            .query_row("SELECT COUNT(*) FROM identity_config", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1);
        std::fs::remove_dir(&snapshot).unwrap();
        std::fs::rename(saved, &snapshot).unwrap();
        drop(state);
        let reopened = crate::vault::Vault::open(&dir).unwrap();
        assert_eq!(reopened.get_mnemonic().unwrap().as_str(), phrase.as_str());
        drop(reopened);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn provider_matrix_key_save_to_master_plan_for_every_provider() {
        // B-001 regression, restated for a single source of truth: the
        // shell no longer keeps its own provider table (`base_for`
        // delegates to `wl_core::ai::catalog`), so what is worth pinning
        // is that every allow-listed id resolves to an endpoint, that a
        // vault key round-trips for it, and that master_plan succeeds
        // with mocked HTTP.
        assert!(
            KNOWN_PROVIDERS.contains(&"openrouter")
                && KNOWN_PROVIDERS.contains(&"google")
                && KNOWN_PROVIDERS.contains(&"bytez.com"),
            "the approved set changed: {KNOWN_PROVIDERS:?}"
        );
        assert!(
            !KNOWN_PROVIDERS.contains(&"qwen"),
            "qwen was removed on 2026-09-27 and must not return via this list"
        );
        let dir = std::env::temp_dir().join(format!("wl-provider-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let state = AppState {
            repos: Repos::new(wl_core::store::open_in_memory().unwrap(), 1),
            identity: std::sync::Mutex::new(None),
            vault: crate::vault::Vault::open(&dir).unwrap(),
            relay_token: std::sync::Mutex::new(None),
            catalog: std::sync::Mutex::new(std::collections::HashMap::new()),
        };
        // A plan that NAMES the goal, since that is the normal case: the
        // compose screen sends raw intent and the architect names it.
        const PLAN: &str = r#"{"title":"Named by the architect","milestones":[{"title":"M","directives":[
            {"title":"D","estimated_minutes":10,"phases":[]}]}]}"#;
        for provider in KNOWN_PROVIDERS {
            let base = base_for(provider);
            assert!(base.is_some(), "no endpoint mapping for {provider}");
            // Every provider the picker offers must also be listable, or
            // the UI would offer a provider it cannot enumerate.
            assert!(
                wl_core::ai::catalog::models_endpoint(provider).is_some(),
                "{provider} has no model-catalog endpoint"
            );
            state
                .vault
                .save_api_key(provider, "sk-test")
                .unwrap_or_else(|_| panic!("vault rejected provider id {provider}"));
            let api_key = vault_api_key(&state, provider).unwrap();
            let adapter = ProviderAdapter::OpenAiCompat {
                base_url: base.unwrap(),
                api_key,
                model: "m".into(),
                json_mode: true,
            };
            let execute = |_: &str, _: &[(String, String)], _: &serde_json::Value| {
                Ok::<String, String>(
                    serde_json::json!({"choices":[{"message":{"content": PLAN}}]}).to_string(),
                )
            };
            let (persisted, _) = AiDispatcher::master_plan(
                &adapter,
                "in n out burger",
                None,
                COMPLEXITY_DEFAULT,
                None,
                execute,
                &state.repos,
                None,
            )
            .unwrap_or_else(|e| panic!("master_plan failed for {provider}: {e}"));
            // The goal is named by the model, not by the raw intent.
            let g = state.repos.goal(&persisted.goal_id).unwrap().unwrap();
            assert_eq!(g.title, "Named by the architect");
            assert_eq!(g.complexity, COMPLEXITY_DEFAULT);
        }
        // Removed providers stay rejected at both layers (no silent fallback).
        for dead in [
            "anthropic",
            "openai",
            "openai-compat",
            "gemini-compat",
            "qwen",
        ] {
            assert!(base_for(dead).is_none(), "{dead} still has an endpoint");
            assert!(
                vault_api_key(&state, dead).is_err(),
                "{dead} still passes the allow-list"
            );
        }
        drop(state);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn manual_goal_seeds_runnable_directive_without_key_or_identity() {
        // B-002 (option b): manual goal creation with NO API key and NO
        // unlocked identity must still yield a runnable directive that
        // the canvas activates on load.
        let repos = Repos::new(wl_core::store::open_in_memory().unwrap(), 1);
        let g = repos
            .create_goal("Ship it", None, None, COMPLEXITY_DEFAULT, None)
            .unwrap();
        seed_first_steps(&repos, &g, None).unwrap();
        let today = today_local();
        let next = repos.next_runnable_directive(&today).unwrap();
        assert!(next.is_some(), "seeded directive is not runnable");
        assert_eq!(next.unwrap().title, "Ship it");
        let engine = Engine::new(&repos, None);
        match engine.current(&today).unwrap() {
            EngineOutcome::DirectiveActive { directive_id, .. } => {
                let active = repos.active_directive().unwrap().unwrap();
                assert_eq!(active.id, directive_id);
            }
            EngineOutcome::Idle => panic!("canvas would show an empty line"),
            _ => {}
        }
    }

    #[test]
    fn manual_goal_supersedes_the_previous_active_goal() {
        // CORE-3: `create_goal` never archived the goals it replaced, so
        // a second manual goal left two rows `active` — and
        // `Repos::active_goal()` is `ORDER BY hlc_timestamp LIMIT 1`, so
        // velocity and recovery bound to the OLDER, superseded goal.
        let repos = Repos::new(wl_core::store::open_in_memory().unwrap(), 1);
        let first = create_seeded_goal(&repos, "First", None, None, COMPLEXITY_DEFAULT, None).unwrap();
        let second = create_seeded_goal(&repos, "Second", None, None, COMPLEXITY_DEFAULT, None).unwrap();
        let active = repos.active_goals_with_progress().unwrap();
        assert_eq!(
            active.len(),
            1,
            "a superseded goal stayed active: {active:?}"
        );
        assert_eq!(active[0].id, second.id);
        assert_eq!(repos.active_goal().unwrap().unwrap().id, second.id);
        // The archived goal keeps its history — only its status moves.
        assert_eq!(
            repos.goal(&first.id).unwrap().unwrap().status,
            GoalStatus::Archived
        );
    }

    #[tokio::test]
    async fn transport_posts_through_the_shared_runtime() {
        let app = axum::Router::new().route(
            "/sync/pull",
            axum::routing::post(|| async { axum::Json(serde_json::json!({"ok": true})) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        tokio::task::spawn_blocking(move || {
            let transport = ReqwestTransport {
                base,
                token: "test-session".into(),
            };
            for _ in 0..3 {
                let response = transport
                    .post("/sync/pull", &serde_json::json!({}))
                    .unwrap();
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&response).unwrap()["ok"],
                    true
                );
            }
        })
        .await
        .unwrap();
        server.abort();
    }
}
