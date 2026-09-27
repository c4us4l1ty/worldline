//! Domain model: goals, milestones, directives, check-ins, bailouts.
//!
//! Extends the PRD §7 schema with the tables the PRD describes in
//! prose but omits from SQL: `goals` (velocity anchor), `directive_phases`
//! (progressive micro-directives, PRD §5.2), `check_ins` (evening
//! audit, PRD §5.4), `bailouts` (escape-hatch ledger, PRD §5.3), and
//! `app_settings` (non-secret config; secrets live in Stronghold only).

use crate::hlc::HlcTimestamp;

// ---------------------------------------------------------------------------
// Goal — the root of the milestone tree; velocity anchor (Δ remaining
// milestones / Δ remaining days).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Goal {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    /// Target completion date (YYYY-MM-DD) — drives target velocity.
    pub target_date: Option<String>,
    /// `active` | `achieved` | `archived`
    pub status: GoalStatus,
    /// The user's own rating of how big this objective is, 1–5.
    ///
    /// Set once, at goal creation, from the compose screen's
    /// *Estimated Complexity* control. It is the **prior** for the
    /// Beta-Bernoulli estimator in [`crate::engine::calibration`] — the
    /// rating is the prior, and every completion or stall afterwards moves
    /// it. Stored per goal rather than per directive because a goal-level
    /// rating is the only one a person can answer before a plan exists.
    pub complexity: i64,
    pub hlc_timestamp: HlcTimestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalStatus {
    Active,
    Achieved,
    Archived,
}

impl GoalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            GoalStatus::Active => "active",
            GoalStatus::Achieved => "achieved",
            GoalStatus::Archived => "archived",
        }
    }
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "active" => Some(GoalStatus::Active),
            "achieved" => Some(GoalStatus::Achieved),
            "archived" => Some(GoalStatus::Archived),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Milestone — PRD §7 `milestones`.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Milestone {
    pub id: String,
    pub goal_id: String,
    pub title: String,
    pub description: Option<String>,
    pub order_index: i64,
    /// `pending` | `active` | `completed` | `demoted`
    pub status: MilestoneStatus,
    pub hlc_timestamp: HlcTimestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MilestoneStatus {
    Pending,
    Active,
    Completed,
    Demoted,
}

impl MilestoneStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            MilestoneStatus::Pending => "pending",
            MilestoneStatus::Active => "active",
            MilestoneStatus::Completed => "completed",
            MilestoneStatus::Demoted => "demoted",
        }
    }
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(MilestoneStatus::Pending),
            "active" => Some(MilestoneStatus::Active),
            "completed" => Some(MilestoneStatus::Completed),
            "demoted" => Some(MilestoneStatus::Demoted),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Directive — the Stackelberg queue unit (PRD §7 `directives`).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Directive {
    pub id: String,
    pub milestone_id: String,
    pub title: String,
    pub execution_context: Option<String>,
    pub estimated_minutes: i64,
    /// 1-based current progressive phase; `progressive_total == 1`
    /// means monolithic (no phase rows).
    pub progressive_step: i64,
    /// Total phases when the directive uses progressive activation.
    pub progressive_total: i64,
    /// `queued` | `active` | `completed` | `blocked` | `skipped`
    pub state: DirectiveState,
    /// YYYY-MM-DD
    pub scheduled_for_date: String,
    /// The one directive that must be `completed` before this one becomes
    /// runnable, or `None` for "nothing blocks this".
    ///
    /// A single prerequisite rather than a list, because that is what the
    /// architect can actually reason about in a plan it just wrote, and
    /// because a chain resolves one link at a time
    /// ([`crate::store::repo::Repos::runnable_directives`]) without any
    /// transitive walk a malformed edge could send into a loop.
    pub depends_on: Option<String>,
    pub hlc_timestamp: HlcTimestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectiveState {
    Queued,
    Active,
    Completed,
    Blocked,
    Skipped,
}

impl DirectiveState {
    pub fn as_str(&self) -> &'static str {
        match self {
            DirectiveState::Queued => "queued",
            DirectiveState::Active => "active",
            DirectiveState::Completed => "completed",
            DirectiveState::Blocked => "blocked",
            DirectiveState::Skipped => "skipped",
        }
    }
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "queued" => Some(DirectiveState::Queued),
            "active" => Some(DirectiveState::Active),
            "completed" => Some(DirectiveState::Completed),
            "blocked" => Some(DirectiveState::Blocked),
            "skipped" => Some(DirectiveState::Skipped),
            _ => None,
        }
    }
}

impl Directive {
    /// Directives longer than 30 minutes activate progressively in
    /// micro-phases to defeat start friction (PRD §5.2).
    pub const PROGRESSIVE_THRESHOLD_MINUTES: i64 = 30;

    pub fn uses_progressive_activation(&self) -> bool {
        self.progressive_total > 1
    }

    /// The title of the currently active micro-phase.
    pub fn phase_title(&self) -> &str {
        // Phases are stored as the full directive title; per-phase
        // instructions live in directive_phases rows (store layer).
        self.title.as_str()
    }
}

// ---------------------------------------------------------------------------
// DirectivePhase — progressive micro-directive (PRD §5.2).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct DirectivePhase {
    pub directive_id: String,
    pub step: i64,
    pub title: String,
    pub instruction: Option<String>,
    pub minutes: i64,
    /// `pending` | `active` | `done`
    pub state: PhaseState,
    pub hlc_timestamp: HlcTimestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseState {
    Pending,
    Active,
    Done,
}

impl PhaseState {
    pub fn as_str(&self) -> &'static str {
        match self {
            PhaseState::Pending => "pending",
            PhaseState::Active => "active",
            PhaseState::Done => "done",
        }
    }
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(PhaseState::Pending),
            "active" => Some(PhaseState::Active),
            "done" => Some(PhaseState::Done),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// CheckIn — evening 30-second audit (PRD §5.4). Objective velocity data,
// never a guilt mechanism.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct CheckIn {
    pub id: String,
    /// YYYY-MM-DD (one check-in per day)
    pub date: String,
    /// `done` | `partial` | `skipped` — objective, non-punitive
    pub outcome: CheckInOutcome,
    pub note: Option<String>,
    pub hlc_timestamp: HlcTimestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckInOutcome {
    Done,
    Partial,
    Skipped,
}

impl CheckInOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            CheckInOutcome::Done => "done",
            CheckInOutcome::Partial => "partial",
            CheckInOutcome::Skipped => "skipped",
        }
    }
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "done" => Some(CheckInOutcome::Done),
            "partial" => Some(CheckInOutcome::Partial),
            "skipped" => Some(CheckInOutcome::Skipped),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// AppSettings — non-secret local config. BYOK API keys and the mnemonic
// live ONLY in the Stronghold vault, never in SQLite.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AppSettings {
    /// Theme: `dark` (default) or `light`.
    pub theme: String,
    /// AI provider: `openrouter` | `google` | `bytez.com` (BYOK).
    pub ai_provider: Option<String>,
    /// User-configurable model ids per tier (Tier 1 architect).
    pub tier1_model: Option<String>,
    /// Tier 2 tactical dispatcher model.
    pub tier2_model: Option<String>,
    /// Relay base URL (e.g. http://127.0.0.1:8080).
    pub relay_url: Option<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            ai_provider: None,
            tier1_model: None,
            tier2_model: None,
            relay_url: None,
        }
    }
}

// ---------------------------------------------------------------------------
// IdentityConfig — PRD §7 `identity_config` (persisted public half only;
// the mnemonic itself never touches SQLite).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct IdentityConfig {
    pub public_key: String,
    pub bip39_mnemonic_verified: bool,
    pub hlc_timestamp: HlcTimestamp,
    /// Onboarding backup-challenge word positions (persisted so the
    /// authoritative re-check is positional, not membership-based).
    pub verify_indices: Vec<usize>,
}

// ---------------------------------------------------------------------------
// Utility: date helpers (YYYY-MM-DD, local dates).
// ---------------------------------------------------------------------------

/// Character budgets for user/AI-supplied text. Anything larger is
/// rejected at the write boundary (`StoreError::Invalid`): unbounded
/// strings bloat the encrypted outbox past the relay's 256 KiB/op cap
/// and wedge sync with an undrainable batch.
pub const MAX_TITLE_CHARS: usize = 500;
pub const MAX_DESCRIPTION_CHARS: usize = 4000;
pub const MAX_CONTEXT_CHARS: usize = 4000;
pub const MAX_NOTE_CHARS: usize = 2000;
pub const MAX_MODEL_ID_CHARS: usize = 256;
/// Bailout context cap from the spec (escape-hatch modal).
pub const MAX_BAILOUT_NOTE_CHARS: usize = 140;
/// A single directive longer than a day is a planning error, not a
/// directive; the bound also keeps phase-rescale arithmetic tame.
pub const MAX_MINUTES: i64 = 1440;
/// AI providers the shell knows how to call (BYOK): exactly the three
/// approved OpenAI-compatible endpoints (PRD-DELTAS #73, amended
/// 2026-09-27 to drop Qwen).
///
/// This is the single allow-list: `vault_api_key` gates on it,
/// `save_settings` rejects a value outside it, and
/// `ai::catalog::base_for` returns `None` for anything else. A provider
/// removed from here therefore fails closed — which is why a persisted
/// value naming a *removed* provider has to be sanitized on read, or
/// every subsequent `settings_save` would fail with "unknown AI
/// provider".
pub const KNOWN_PROVIDERS: &[&str] = &["openrouter", "google", "bytez.com"];

/// Rejects blank or over-budget text at write boundaries.
pub fn check_text(field: &'static str, value: &str, max_chars: usize) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{field} must not be blank"));
    }
    if value.chars().count() > max_chars {
        return Err(format!("{field} exceeds {max_chars} characters"));
    }
    Ok(())
}

/// Rejects over-budget optional text (`None` always passes).
pub fn check_optional_text(
    field: &'static str,
    value: Option<&str>,
    max_chars: usize,
) -> Result<(), String> {
    if let Some(v) = value {
        if v.chars().count() > max_chars {
            return Err(format!("{field} exceeds {max_chars} characters"));
        }
    }
    Ok(())
}

/// Strict calendar-date check (`YYYY-MM-DD`, real month/day).
/// Shape-exact (chrono alone accepts `2026-9-8`, which would corrupt
/// the lexicographic date ordering every query relies on).
pub fn check_date(field: &'static str, date: &str) -> Result<(), String> {
    let bytes = date.as_bytes();
    let shaped = bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
    if !shaped || chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err() {
        return Err(format!("{field} must be a YYYY-MM-DD calendar date"));
    }
    Ok(())
}

/// Directive/phase minute bounds shared by manual, AI, and reschedule paths.
pub fn check_minutes(field: &'static str, minutes: i64) -> Result<(), String> {
    if minutes <= 0 || minutes > MAX_MINUTES {
        return Err(format!("{field} must be within 1–{MAX_MINUTES} minutes"));
    }
    Ok(())
}

/// Lowest / highest / default *Estimated Complexity* rating.
///
/// The scale is five stops because that is what survives a 420px control
/// built from five buttons (see `.wl-calibration`): each stop keeps a 44px
/// touch target, which seven would not. The default is the middle stop, so
/// a user who never touches the slider contributes an uninformative Beta
/// rather than a confident wrong one.
pub const COMPLEXITY_MIN: i64 = 1;
pub const COMPLEXITY_MAX: i64 = 5;
pub const COMPLEXITY_DEFAULT: i64 = 3;

/// Word for a rating, or `None` when the rating is out of range.
///
/// Returns `Option` rather than a placeholder so a corrupt replicated
/// `goals.complexity` renders nothing rather than labelling 3/5 work
/// "standard" — the same rule the entropy log follows for an unrecognised
/// bailout reason.
pub fn complexity_label(complexity: i64) -> Option<&'static str> {
    if !(COMPLEXITY_MIN..=COMPLEXITY_MAX).contains(&complexity) {
        return None;
    }
    Some(COMPLEXITY_WORDS[(complexity - COMPLEXITY_MIN) as usize])
}

/// The five stops, in rating order. A table rather than a derivation
/// because the wording is a product decision: the estimator is primed by
/// the *number* while the user is choosing by the *word*, and "light+" has
/// no single-word synonym.
const COMPLEXITY_WORDS: [&str; 5] = ["light", "light+", "standard", "heavy", "deep"];

/// Rejects a rating outside 1–5 at the write boundary.
pub fn check_complexity(field: &'static str, complexity: i64) -> Result<(), String> {
    if !(COMPLEXITY_MIN..=COMPLEXITY_MAX).contains(&complexity) {
        return Err(format!(
            "{field} must be within {COMPLEXITY_MIN}–{COMPLEXITY_MAX}"
        ));
    }
    Ok(())
}

/// Coerces a rating into 1–5, for read paths over replicated rows.
///
/// A peer can write `complexity = 0` or `99`: the CHECK constraint is a
/// local write boundary, and `apply_upsert` reads the value out of a sealed
/// payload with no such check. Rather than reject the op — which would
/// quarantine an entire goal over one field and wedge sync — the value is
/// clamped on read, because a nonsensical estimate of "standard" is a far
/// better outcome than a goal that cannot be pulled.
pub fn clamp_complexity(complexity: i64) -> i64 {
    complexity.clamp(COMPLEXITY_MIN, COMPLEXITY_MAX)
}

/// Today's local date as YYYY-MM-DD.
pub fn today_local() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Adds `days` to a YYYY-MM-DD date string.
pub fn date_plus_days(date: &str, days: i64) -> Option<String> {
    let d = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    d.checked_add_signed(chrono::Duration::try_days(days)?)
        .map(|nd| nd.format("%Y-%m-%d").to_string())
}

/// Days between two YYYY-MM-DD dates (a - b).
pub fn days_between(a: &str, b: &str) -> Option<i64> {
    let da = chrono::NaiveDate::parse_from_str(a, "%Y-%m-%d").ok()?;
    let db = chrono::NaiveDate::parse_from_str(b, "%Y-%m-%d").ok()?;
    Some((da - db).num_days())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directive_progressive_defaults() {
        let d = Directive {
            id: "d1".into(),
            milestone_id: "m1".into(),
            title: "Write 300 words on Section 2.1".into(),
            execution_context: None,
            estimated_minutes: 45,
            progressive_step: 1,
            progressive_total: 3,
            state: DirectiveState::Queued,
            scheduled_for_date: today_local(),
            depends_on: None,
            hlc_timestamp: HlcTimestamp {
                physical: 1,
                counter: 0,
                device: 1,
            },
        };
        assert!(d.uses_progressive_activation());
        let mut monolithic = d.clone();
        monolithic.progressive_total = 1;
        assert!(!monolithic.uses_progressive_activation());
        assert_eq!(Directive::PROGRESSIVE_THRESHOLD_MINUTES, 30);
    }

    #[test]
    fn date_helpers() {
        assert_eq!(date_plus_days("2026-09-13", 1).unwrap(), "2026-09-14");
        assert_eq!(date_plus_days("2026-02-28", 1).unwrap(), "2026-03-01"); // non-leap
        assert_eq!(days_between("2026-09-13", "2026-09-10").unwrap(), 3);
        assert!(days_between("2026-09-13", "garbage").is_none());
        assert!(today_local().len() == 10);
    }

    #[test]
    fn date_plus_days_extremes_return_none_not_panic() {
        assert_eq!(date_plus_days("2026-09-13", 0).unwrap(), "2026-09-13");
        let min = chrono::NaiveDate::MIN.format("%Y-%m-%d").to_string();
        let max = chrono::NaiveDate::MAX.format("%Y-%m-%d").to_string();
        assert_eq!(date_plus_days(&min, 0), Some(min.clone()));
        assert_eq!(date_plus_days(&max, 0), Some(max.clone()));
        assert_eq!(date_plus_days(&min, -1), None);
        assert_eq!(date_plus_days(&max, 1), None);
        assert_eq!(date_plus_days("2026-09-13", i64::MAX), None);
        assert_eq!(date_plus_days("2026-09-13", i64::MIN), None);
        assert_eq!(
            date_plus_days("2024-03-01", -1).as_deref(),
            Some("2024-02-29")
        );
        assert_eq!(date_plus_days("garbage", 1), None);
    }

    #[test]
    fn enums_roundtrip() {
        for s in ["active", "achieved", "archived"] {
            assert_eq!(GoalStatus::from_str(s).unwrap().as_str(), s);
        }
        for s in ["pending", "active", "completed", "demoted"] {
            assert_eq!(MilestoneStatus::from_str(s).unwrap().as_str(), s);
        }
        for s in ["queued", "active", "completed", "blocked", "skipped"] {
            assert_eq!(DirectiveState::from_str(s).unwrap().as_str(), s);
        }
        for s in ["done", "partial", "skipped"] {
            assert_eq!(CheckInOutcome::from_str(s).unwrap().as_str(), s);
        }
        // No round-trip for the three bailout reasons: `BailoutReason` went
        // with the escape hatch (2026-09-27) and there is no decoder left
        // to pin. The `bailouts` TABLE stays in the schema and is no longer
        // written — same terms as `app_settings.hotkey` (delta 175).
        assert!(GoalStatus::from_str("nope").is_none());
    }

    #[test]
    fn write_boundary_text_budgets() {
        assert!(check_text("title", "Ship it", MAX_TITLE_CHARS).is_ok());
        assert!(check_text("title", "   ", MAX_TITLE_CHARS).is_err());
        assert!(check_text("title", "", MAX_TITLE_CHARS).is_err());
        assert!(check_text("title", &"x".repeat(MAX_TITLE_CHARS), MAX_TITLE_CHARS).is_ok());
        assert!(check_text("title", &"x".repeat(MAX_TITLE_CHARS + 1), MAX_TITLE_CHARS).is_err());
        // Multi-byte chars count as characters, not bytes.
        assert!(check_text("t", &"é".repeat(MAX_TITLE_CHARS), MAX_TITLE_CHARS).is_ok());
        assert!(check_optional_text("d", None, 4).is_ok());
        assert!(check_optional_text("d", Some("ok"), 4).is_ok());
        assert!(check_optional_text("d", Some("toolong"), 4).is_err());
    }

    #[test]
    fn write_boundary_dates_and_minutes() {
        assert!(check_date("d", "2026-09-18").is_ok());
        assert!(check_date("d", "2024-02-29").is_ok());
        for bad in [
            "",
            "garbage",
            "2026-13-01",
            "2026-02-30",
            "2026-9-8",
            " 2026-09-18",
        ] {
            assert!(check_date("d", bad).is_err(), "{bad:?} must be rejected");
        }
        assert!(check_minutes("m", 1).is_ok());
        assert!(check_minutes("m", MAX_MINUTES).is_ok());
        for bad in [0, -5, MAX_MINUTES + 1, i64::MAX] {
            assert!(check_minutes("m", bad).is_err());
        }
    }
}
