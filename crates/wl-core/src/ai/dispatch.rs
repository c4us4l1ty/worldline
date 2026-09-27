//! Hierarchical AI dispatcher (PRD §4): calls BYOK providers via
//! pluggable HTTP adapters, parses JSON-schema-constrained responses,
//! and persists plans/directives through the repos.

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::identity::Identity;
use crate::domain::*;
use crate::store::repo::Repos;
use crate::store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("response was not valid JSON: {0}")]
    BadJson(String),
    #[error("missing field {0}")]
    MissingField(&'static str),
    #[error("invalid value for {field}: {why}")]
    Invalid { field: &'static str, why: String },
    #[error("no AI provider configured (BYOK required)")]
    NoProvider,
}

// ---------------------------------------------------------------------------
// Wire adapters
// ---------------------------------------------------------------------------

/// BYOK provider endpoints. Model ids are user settings, never baked in.
///
/// The supported provider set is exactly `openrouter | google |
/// bytez.com` (PRD-DELTAS #73, amended 2026-09-27) — every one speaks
/// OpenAI-compatible chat completions, so a single variant plus a
/// per-provider base URL covers the whole matrix (the old
/// Anthropic-native variant is gone).
///
/// The key rides in `Zeroizing<String>` end-to-end (vault → adapter →
/// request): dropping the adapter wipes it. Two residual copies are
/// unavoidable and short-lived — the `Authorization`/`x-api-key`
/// header strings handed to the HTTP layer, and whatever the HTTP
/// client retains for the connection. Keys are never logged: `Debug`
/// is redacted by hand (a derived `Debug` would print key material
/// into any `{:?}` error path).
#[derive(Clone)]
pub enum ProviderAdapter {
    /// OpenAI-compatible chat completions (OpenRouter, Google Gemini
    /// compat, bytez gateway). The shell resolves the provider id to
    /// its base URL; the adapter only carries it.
    OpenAiCompat {
        base_url: String,
        api_key: Zeroizing<String>,
        model: String,
        /// Whether to send `response_format: {"type":"json_object"}`.
        ///
        /// This is per-model, not per-provider. Of OpenRouter's 443
        /// text-output models only 391 advertise the parameter, and a
        /// handful (`openrouter/auto`, `openrouter/fusion`, …) advertise
        /// none at all, so the field is not universally accepted and a
        /// model that rejects it fails on the request *shape* — with
        /// nothing on screen to explain it.
        ///
        /// How universal that failure is, honestly: a live probe on
        /// 2026-09-27 showed OpenRouter *accepting* the field on
        /// `nvidia/nemotron-3.5-lightning:free`, which does not advertise
        /// it. So the metadata marks unsupported parameters, not ones
        /// guaranteed to 400, and this flag removes a field the provider
        /// has told us it does not support rather than fixing a
        /// universal break.
        ///
        /// Derived from the live catalog via
        /// [`crate::ai::catalog::json_mode_for`], which only turns it
        /// *off* on positive evidence. Turning it off is safe: both
        /// prompts already demand JSON in prose and the parser strips
        /// fences and extracts the first balanced object, so the field
        /// is a preference, not the contract.
        json_mode: bool,
    },
}

impl std::fmt::Debug for ProviderAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderAdapter::OpenAiCompat {
                base_url, model, ..
            } => f
                .debug_struct("OpenAiCompat")
                .field("base_url", base_url)
                .field("api_key", &"[redacted]")
                .field("model", model)
                .finish(),
        }
    }
}

impl ProviderAdapter {
    /// HTTP request (url, headers, json body) — executed by the shell
    /// layer (reqwest is native-only; the wasm UI never holds keys).
    pub fn request(
        &self,
        system: &str,
        user: &str,
    ) -> (String, Vec<(String, String)>, serde_json::Value) {
        match self {
            ProviderAdapter::OpenAiCompat {
                base_url,
                api_key,
                model,
                json_mode,
            } => {
                let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
                let mut body = serde_json::json!({
                    "model": model,
                    "messages": [
                        {"role": "system", "content": system},
                        {"role": "user", "content": user}
                    ],
                    "temperature": 0.4,
                });
                if *json_mode {
                    body["response_format"] = serde_json::json!({"type": "json_object"});
                }
                let headers = vec![
                    (
                        "Authorization".into(),
                        format!("Bearer {}", api_key.as_str()),
                    ),
                    ("Content-Type".into(), "application/json".into()),
                ];
                (url, headers, body)
            }
        }
    }

    /// Extracts the assistant text from a provider response body.
    pub fn extract_text(&self, body: &serde_json::Value) -> Option<String> {
        match self {
            ProviderAdapter::OpenAiCompat { .. } => body["choices"][0]["message"]["content"]
                .as_str()
                .map(Into::into),
        }
    }
}

// ---------------------------------------------------------------------------
// Structured plan outputs (Tier 1 / Tier 2 shared shapes)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PhaseDraft {
    pub title: String,
    #[serde(default)]
    pub instruction: Option<String>,
    pub minutes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DirectiveDraft {
    pub title: String,
    #[serde(default)]
    pub execution_context: Option<String>,
    pub estimated_minutes: i64,
    #[serde(default)]
    pub phases: Vec<PhaseDraft>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MilestoneDraft {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub directives: Vec<DirectiveDraft>,
}

/// Full Tier-1 result: the goal the architect named, plus its milestone
/// hierarchy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanResult {
    /// Goal title the architect derived from the user's intent.
    ///
    /// The compose screen collects ONE free-text field ("in n out
    /// burger"), not a title, so the model is what names the goal. Both
    /// fields are `#[serde(default)]` so a response that omits them
    /// still parses and `persist_plan` falls back — a model that returns
    /// a good plan but no title must not cost the user the whole request.
    #[serde(default)]
    pub title: Option<String>,
    /// Optional longer framing for the goal. Rarely returned; when it is,
    /// it replaces the raw intent as the goal description.
    #[serde(default)]
    pub description: Option<String>,
    pub milestones: Vec<MilestoneDraft>,
}

/// Tier-2 result: 1–3 directives for today.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BriefingResult {
    pub directives: Vec<DirectiveDraft>,
    /// IDs of the directive rows this briefing persisted (empty when
    /// persistence was skipped/failed upstream — B-005 honesty).
    /// Not serialized on the wire contract; set by the dispatcher.
    #[serde(default, skip_serializing)]
    pub created_ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// Response parsing (strict, schema-validated)
// ---------------------------------------------------------------------------

/// Strips markdown code fences that models love to add.
fn strip_fences(s: &str) -> &str {
    let t = s.trim();
    let t = t
        .strip_prefix("```json")
        .or(t.strip_prefix("```"))
        .unwrap_or(t);
    t.strip_suffix("```").unwrap_or(t)
}

/// Extracts the first balanced JSON object/array from a string.
fn extract_json_block(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let start = bytes.iter().position(|b| *b == b'{' || *b == b'[')?;
    let open = bytes[start];
    let close = if open == b'{' { b'}' } else { b']' };
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (i, &b) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            x if x == open => depth += 1,
            x if x == close => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[start..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_DIRECTIVES_PER_MILESTONE: usize = 32;
/// Prompt-input budgets: fail fast BEFORE spending a BYOK call on a
/// body the provider would truncate or bill absurdly.
///
/// Tier-1's input (the compose screen's free-text intent) is bounded by
/// `domain::MAX_DESCRIPTION_CHARS` instead of a separate constant — it
/// is one prose field now, so the store's own budget is the honest cap.
/// Tier-2 still takes a distinct constraints argument, hence its own.
const MAX_AI_CONSTRAINTS_CHARS: usize = 8 * 1024;

fn validate_response_size(raw: &str) -> Result<(), DispatchError> {
    if raw.len() > MAX_RESPONSE_BYTES {
        return Err(DispatchError::Invalid {
            field: "response",
            why: "response exceeds 256 KiB".into(),
        });
    }
    Ok(())
}

fn validate_text(field: &'static str, text: &str) -> Result<(), DispatchError> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(DispatchError::Invalid {
            field,
            why: "text exceeds 16 KiB".into(),
        });
    }
    Ok(())
}

pub fn parse_plan(raw: &str) -> Result<PlanResult, DispatchError> {
    validate_response_size(raw)?;
    let cleaned = strip_fences(raw);
    let block = extract_json_block(cleaned)
        .ok_or_else(|| DispatchError::BadJson("no JSON object found".into()))?;
    let plan: PlanResult =
        serde_json::from_str(block).map_err(|e| DispatchError::BadJson(e.to_string()))?;
    validate_plan(&plan)?;
    Ok(plan)
}

pub fn parse_briefing(raw: &str) -> Result<BriefingResult, DispatchError> {
    validate_response_size(raw)?;
    let cleaned = strip_fences(raw);
    let block = extract_json_block(cleaned)
        .ok_or_else(|| DispatchError::BadJson("no JSON object found".into()))?;
    let b: BriefingResult =
        serde_json::from_str(block).map_err(|e| DispatchError::BadJson(e.to_string()))?;
    validate_briefing(&b)?;
    Ok(b)
}

fn validate_briefing(b: &BriefingResult) -> Result<(), DispatchError> {
    if b.directives.is_empty() {
        return Err(DispatchError::Invalid {
            field: "directives",
            why: "must contain 1–3 directives".into(),
        });
    }
    if b.directives.len() > 3 {
        return Err(DispatchError::Invalid {
            field: "directives",
            why: "more than 3 directives for one day".into(),
        });
    }
    for d in &b.directives {
        validate_directive(d)?;
    }
    Ok(())
}

fn validate_plan(p: &PlanResult) -> Result<(), DispatchError> {
    // The architect names the goal, so its title is validated like every
    // other model-authored string. A title that is present but blank is
    // NOT an error: `persist_plan` reads that as "no title offered" and
    // falls back to the user's own words. Rejecting here would turn a
    // cosmetic omission into a failed BYOK call.
    if let Some(t) = p.title.as_deref() {
        if !t.trim().is_empty() {
            validate_text("plan.title", t)?;
            if t.chars().count() > crate::domain::MAX_TITLE_CHARS {
                return Err(DispatchError::Invalid {
                    field: "plan.title",
                    why: format!("over {} characters", crate::domain::MAX_TITLE_CHARS),
                });
            }
        }
    }
    if let Some(d) = p.description.as_deref() {
        if !d.trim().is_empty() {
            validate_text("plan.description", d)?;
        }
    }
    // CORE-7(c): the guard allowed 1 milestone while the message claimed
    // "2–5". A model returning a single-milestone plan got a rejection
    // that contradicted the code, and a 1-milestone plan that *was*
    // valid failed schema review against this string. The code is the
    // intended contract (a lone milestone is a legitimate plan), so the
    // message now matches it.
    if p.milestones.is_empty() || p.milestones.len() > 5 {
        return Err(DispatchError::Invalid {
            field: "milestones",
            why: "1–5 milestones required".into(),
        });
    }
    for m in &p.milestones {
        validate_text("milestone.title", &m.title)?;
        validate_text(
            "milestone.description",
            m.description.as_deref().unwrap_or(""),
        )?;
        if m.directives.len() > MAX_DIRECTIVES_PER_MILESTONE {
            return Err(DispatchError::Invalid {
                field: "milestone.directives",
                why: "more than 32 directives".into(),
            });
        }
        if m.title.trim().is_empty() {
            return Err(DispatchError::Invalid {
                field: "milestone.title",
                why: "empty".into(),
            });
        }
        for d in &m.directives {
            validate_directive(d)?;
        }
    }
    Ok(())
}

fn validate_directive(d: &DirectiveDraft) -> Result<(), DispatchError> {
    validate_text("directive.title", &d.title)?;
    validate_text(
        "execution_context",
        d.execution_context.as_deref().unwrap_or(""),
    )?;
    if d.title.trim().is_empty() {
        return Err(DispatchError::MissingField("directive.title"));
    }
    if d.estimated_minutes <= 0 || d.estimated_minutes > crate::domain::MAX_MINUTES {
        return Err(DispatchError::Invalid {
            field: "estimated_minutes",
            why: format!("must be within 1–{} minutes", crate::domain::MAX_MINUTES),
        });
    }
    if !d.phases.is_empty() {
        if !(2..=4).contains(&d.phases.len()) {
            return Err(DispatchError::Invalid {
                field: "phases",
                why: "2–4 phases required".into(),
            });
        }
        for p in &d.phases {
            validate_text("phase.title", &p.title)?;
            validate_text("phase.instruction", p.instruction.as_deref().unwrap_or(""))?;
            if p.title.trim().is_empty() || !(1..=25).contains(&p.minutes) {
                return Err(DispatchError::Invalid {
                    field: "phases",
                    why: "nonempty titles and minutes in 1–25 required".into(),
                });
            }
        }
        let sum: i128 = d.phases.iter().map(|p| i128::from(p.minutes)).sum();
        if sum <= 0 {
            return Err(DispatchError::Invalid {
                field: "phases",
                why: "total minutes must be positive".into(),
            });
        }
        // The phase table replaces the headline estimate at persist
        // time (`persist_plan`), so a sum that contradicts the estimate
        // silently rewrites it (e.g. 60m → 10m). Reject sums outside a
        // 2× band around the estimate.
        if sum * 2 < i128::from(d.estimated_minutes) || sum > i128::from(d.estimated_minutes) * 2 {
            return Err(DispatchError::Invalid {
                field: "phases",
                why: format!(
                    "phase minutes sum to {sum}, contradicting the {est}m estimate",
                    est = d.estimated_minutes
                ),
            });
        }
    } else if d.estimated_minutes > Directive::PROGRESSIVE_THRESHOLD_MINUTES {
        return Err(DispatchError::Invalid {
            field: "phases",
            why: format!(
                "{}min directive requires progressive phases",
                d.estimated_minutes
            ),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Persistence of parsed results (manual fallback path shares this)
// ---------------------------------------------------------------------------

/// Derives a goal title from the user's raw intent, for the case where
/// the architect did not offer one.
///
/// The compose screen collects one free-text field, so this text can be
/// a fragment ("in n out burger") or several paragraphs. Only the first
/// non-empty line is used, and it is capped at `MAX_TITLE_CHARS` —
/// `create_goal` enforces that bound, so an uncapped fallback would
/// reject the whole plan over a title the model simply omitted.
pub fn fallback_title(intent: &str) -> String {
    let first = intent.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let trimmed = first.trim();
    if trimmed.chars().count() <= crate::domain::MAX_TITLE_CHARS {
        return trimmed.to_string();
    }
    let clipped: String = trimmed
        .chars()
        .take(crate::domain::MAX_TITLE_CHARS)
        .collect();
    // Prefer to end on a word boundary so the title does not break
    // mid-word; fall back to the hard clip if the first "word" is longer
    // than the whole budget.
    match clipped.rfind(' ') {
        Some(i) if i > 0 => clipped[..i].trim_end().to_string(),
        _ => clipped,
    }
}

/// Persists a Tier-1 plan: goal, milestones, directives.
///
/// `goal_spec`: `(fallback_title, description, target_date)` — a new
/// goal is created from it. (Restructuring an existing goal re-uses this
/// with the goal closed and re-planned under a fresh goal.)
///
/// Title precedence: the architect's own `plan.title` wins, because the
/// user gave it free text rather than a name and naming the goal is the
/// model's job. `goal_spec`'s title is the fallback for a response that
/// omitted one, and `"Untitled goal"` is the last resort so a goal is
/// never created nameless.
pub fn persist_plan(
    repos: &Repos,
    goal_spec: Option<(&str, Option<&str>, Option<&str>)>,
    plan: &PlanResult,
    identity: Option<&Identity>,
) -> Result<String, DispatchError> {
    validate_plan(plan)?;
    let (title, desc, target) = match goal_spec {
        Some((fallback, desc, target)) => {
            let title = plan
                .title
                .as_deref()
                .filter(|t| !t.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| fallback_title(fallback));
            // The model's description is the goal's framing; the spec's is
            // whatever the caller already had. Prefer the model's when it
            // actually said something.
            let desc = plan
                .description
                .as_deref()
                .filter(|d| !d.trim().is_empty())
                .or(desc)
                .map(str::to_string);
            let title = if title.trim().is_empty() {
                "Untitled goal".to_string()
            } else {
                title
            };
            (title, desc, target.map(str::to_string))
        }
        None => {
            let title = plan
                .title
                .as_deref()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or("Untitled goal")
                .to_string();
            (title, None, None)
        }
    };
    let goal_id = repos
        .create_goal(&title, desc.as_deref(), target.as_deref(), identity)?
        .id;
    // CORE-3: a new plan supersedes the previous one. Without this every
    // `persist_plan` left the old goal `active` forever, so goals
    // accumulated in the `active` state and `active_goal()`'s `LIMIT 1`
    // became an arbitrary tie-break. Runs AFTER the new goal exists so
    // `keep` can never archive the goal we just created.
    repos.archive_other_active_goals(&goal_id, identity)?;
    for (i, m) in plan.milestones.iter().enumerate() {
        let ms = repos.create_milestone(
            &goal_id,
            &m.title,
            m.description.as_deref(),
            i as i64,
            identity,
        )?;
        for d in &m.directives {
            let phases = super::prompt::phases_of(d);
            let total = if phases.is_empty() {
                1
            } else {
                phases.len() as i64
            };
            let mins = if d.estimated_minutes > Directive::PROGRESSIVE_THRESHOLD_MINUTES
                && !phases.is_empty()
            {
                phases.iter().map(|(_, _, m)| m).sum()
            } else {
                d.estimated_minutes
            };
            repos.create_directive(
                &ms.id,
                &d.title,
                d.execution_context.as_deref(),
                mins,
                total,
                &crate::domain::today_local(),
                &phases,
                identity,
            )?;
        }
    }
    Ok(goal_id)
}

/// Persists a Tier-2 briefing into the active goal's next milestone.
pub fn persist_briefing(
    repos: &Repos,
    brief: &BriefingResult,
    date: &str,
    identity: Option<&Identity>,
) -> Result<Vec<String>, DispatchError> {
    validate_briefing(brief)?;
    let goal = repos
        .active_goal()?
        .ok_or(StoreError::NotFound("no active goal".into()))?;
    let ms = repos
        .next_pending_milestone(&goal.id)?
        .ok_or(StoreError::NotFound("no pending milestone".into()))?;
    let mut ids = Vec::new();
    for d in &brief.directives {
        let phases = super::prompt::phases_of(d);
        let total = if phases.is_empty() {
            1
        } else {
            phases.len() as i64
        };
        let mins = if d.estimated_minutes > Directive::PROGRESSIVE_THRESHOLD_MINUTES
            && !phases.is_empty()
        {
            phases.iter().map(|(_, _, m)| m).sum()
        } else {
            d.estimated_minutes
        };
        let dir = repos.create_directive(
            &ms.id,
            &d.title,
            d.execution_context.as_deref(),
            mins,
            total,
            date,
            &phases,
            identity,
        )?;
        ids.push(dir.id);
    }
    Ok(ids)
}

// ---------------------------------------------------------------------------
// Dispatcher facade
// ---------------------------------------------------------------------------

/// Orchestrates a tier call: build prompt → (shell executes HTTP) →
/// parse → persist. The HTTP hop is injected so this type stays
/// platform-clean and unit-testable.
pub struct AiDispatcher;

impl AiDispatcher {
    /// Tier 1 — Master Architect. Returns `(goal_id, plan)`.
    ///
    /// `intent` is the user's raw free text from the compose screen —
    /// not a title. It may be a fragment ("in n out burger"), a
    /// paragraph, or contain the constraints that used to live in their
    /// own field. The architect names the goal from it; see
    /// [`persist_plan`] for title precedence.
    #[allow(clippy::too_many_arguments)] // explicit params keep the injected-HTTP design testable.
    pub fn master_plan<
        F: Fn(&str, &[(String, String)], &serde_json::Value) -> Result<String, String>,
    >(
        provider: &ProviderAdapter,
        intent: &str,
        target_date: Option<&str>,
        execute: F,
        repos: &Repos,
        identity: Option<&Identity>,
    ) -> Result<(String, PlanResult), DispatchError> {
        // Validate BEFORE the billable HTTP hop; persistence re-checks
        // at the write boundary. The bound is the description budget,
        // not the title budget: this is prose, not a name.
        crate::domain::check_text("intent", intent, crate::domain::MAX_DESCRIPTION_CHARS).map_err(
            |why| DispatchError::Invalid {
                field: "intent",
                why,
            },
        )?;
        if let Some(t) = target_date {
            crate::domain::check_date("target date", t).map_err(|why| DispatchError::Invalid {
                field: "target_date",
                why,
            })?;
        }
        let (url, headers, body) = provider.request(
            &super::prompt::tier1_system(),
            &super::prompt::tier1_user(intent, target_date),
        );
        let raw = execute(&url, &headers, &body).map_err(DispatchError::BadJson)?;
        validate_response_size(&raw)?;
        let resp: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| DispatchError::BadJson(e.to_string()))?;
        let text = provider
            .extract_text(&resp)
            .ok_or_else(|| DispatchError::BadJson("no content in response".into()))?;
        let plan = parse_plan(&text)?;
        // `intent` doubles as the title fallback: if the model named the
        // goal, this is ignored.
        let goal_id = persist_plan(repos, Some((intent, None, target_date)), &plan, identity)?;
        Ok((goal_id, plan))
    }

    /// Tier 2 — Tactical Dispatcher. Returns created directive ids.
    pub fn morning_briefing<
        F: Fn(&str, &[(String, String)], &serde_json::Value) -> Result<String, String>,
    >(
        provider: &ProviderAdapter,
        today: &str,
        constraints: &str,
        velocity_json: &str,
        execute: F,
        repos: &Repos,
        identity: Option<&Identity>,
    ) -> Result<BriefingResult, DispatchError> {
        crate::domain::check_date("today", today).map_err(|why| DispatchError::Invalid {
            field: "today",
            why,
        })?;
        if constraints.chars().count() > MAX_AI_CONSTRAINTS_CHARS {
            return Err(DispatchError::Invalid {
                field: "constraints",
                why: format!("constraints exceed {MAX_AI_CONSTRAINTS_CHARS} characters"),
            });
        }
        let user_prompt = super::prompt::tier2_user(repos, today, constraints, velocity_json)?;
        let (url, headers, body) = provider.request(&super::prompt::tier2_system(), &user_prompt);
        let raw = execute(&url, &headers, &body).map_err(DispatchError::BadJson)?;
        validate_response_size(&raw)?;
        let resp: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| DispatchError::BadJson(e.to_string()))?;
        let text = provider
            .extract_text(&resp)
            .ok_or_else(|| DispatchError::BadJson("no content in response".into()))?;
        let brief = parse_briefing(&text)?;
        let created_ids = persist_briefing(repos, &brief, today, identity)?;
        Ok(BriefingResult {
            directives: brief.directives,
            created_ids,
        })
    }
}
