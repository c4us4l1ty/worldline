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
/// The key rides in `Zeroizing<String>` end-to-end (vault → adapter →
/// request): dropping the adapter wipes it. Two residual copies are
/// unavoidable and short-lived — the `Authorization`/`x-api-key`
/// header strings handed to the HTTP layer, and whatever the HTTP
/// client retains for the connection. Keys are never logged: `Debug`
/// is redacted by hand (a derived `Debug` would print key material
/// into any `{:?}` error path).
#[derive(Clone)]
pub enum ProviderAdapter {
    /// OpenAI-compatible chat completions (OpenAI, OpenRouter,
    /// Gemini compat, LM Studio, Ollama…).
    OpenAiCompat {
        base_url: String,
        api_key: Zeroizing<String>,
        model: String,
    },
    /// Anthropic messages API.
    Anthropic {
        api_key: Zeroizing<String>,
        model: String,
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
            ProviderAdapter::Anthropic { model, .. } => f
                .debug_struct("Anthropic")
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
            } => {
                let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
                let body = serde_json::json!({
                    "model": model,
                    "messages": [
                        {"role": "system", "content": system},
                        {"role": "user", "content": user}
                    ],
                    "temperature": 0.4,
                    "response_format": {"type": "json_object"}
                });
                let headers = vec![
                    (
                        "Authorization".into(),
                        format!("Bearer {}", api_key.as_str()),
                    ),
                    ("Content-Type".into(), "application/json".into()),
                ];
                (url, headers, body)
            }
            ProviderAdapter::Anthropic { api_key, model } => {
                let body = serde_json::json!({
                    "model": model,
                    "max_tokens": 4096,
                    "system": system,
                    "messages": [{"role": "user", "content": user}]
                });
                let headers = vec![
                    ("x-api-key".into(), api_key.as_str().to_owned()),
                    ("anthropic-version".into(), "2023-06-01".into()),
                    ("Content-Type".into(), "application/json".into()),
                ];
                (
                    "https://api.anthropic.com/v1/messages".into(),
                    headers,
                    body,
                )
            }
        }
    }

    /// Extracts the assistant text from a provider response body.
    pub fn extract_text(&self, body: &serde_json::Value) -> Option<String> {
        match self {
            ProviderAdapter::OpenAiCompat { .. } => body["choices"][0]["message"]["content"]
                .as_str()
                .map(Into::into),
            ProviderAdapter::Anthropic { .. } => {
                body["content"][0]["text"].as_str().map(Into::into)
            }
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

/// Full Tier-1 result: milestone hierarchy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanResult {
    pub milestones: Vec<MilestoneDraft>,
}

/// Tier-2 result: 1–3 directives for today.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BriefingResult {
    pub directives: Vec<DirectiveDraft>,
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
    if p.milestones.is_empty() || p.milestones.len() > 5 {
        return Err(DispatchError::Invalid {
            field: "milestones",
            why: "2–5 milestones required".into(),
        });
    }
    for m in &p.milestones {
        validate_text("milestone.title", &m.title)?;
        validate_text("milestone.description", m.description.as_deref().unwrap_or(""))?;
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

#[cfg(test)]
mod validation_regressions {
    use super::*;

    #[test]
    fn phase_validation_rejects_unrepresentable_totals_without_panicking() {
        let draft = DirectiveDraft {
            title: "Boundary estimate".into(),
            execution_context: None,
            estimated_minutes: i64::MAX,
            phases: vec![
                PhaseDraft {
                    title: "First".into(),
                    instruction: None,
                    minutes: i64::MAX,
                },
                PhaseDraft {
                    title: "Second".into(),
                    instruction: None,
                    minutes: 1,
                },
            ],
        };
        assert!(validate_directive(&draft).is_err());
    }
}

fn validate_directive(d: &DirectiveDraft) -> Result<(), DispatchError> {
    validate_text("directive.title", &d.title)?;
    validate_text("execution_context", d.execution_context.as_deref().unwrap_or(""))?;
    if d.title.trim().is_empty() {
        return Err(DispatchError::MissingField("directive.title"));
    }
    if d.estimated_minutes <= 0 {
        return Err(DispatchError::Invalid {
            field: "estimated_minutes",
            why: "must be positive".into(),
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

/// Persists a Tier-1 plan: goal, milestones, directives.
///
/// `goal_spec`: `(title, description, target_date)` — a new goal is
/// created from it. (Restructuring an existing goal re-uses this with
/// the goal closed and re-planned under a fresh goal.)
pub fn persist_plan(
    repos: &Repos,
    goal_spec: Option<(&str, Option<&str>, Option<&str>)>,
    plan: &PlanResult,
    identity: Option<&Identity>,
) -> Result<String, DispatchError> {
    let goal_id = match goal_spec {
        Some((title, desc, target)) => repos.create_goal(title, desc, target, identity)?.id,
        None => repos.create_goal("Untitled goal", None, None, identity)?.id,
    };
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
    /// Tier 1 — Master Architect. Returns goal id.
    #[allow(clippy::too_many_arguments)] // explicit params keep the injected-HTTP design testable.
    pub fn master_plan<
        F: Fn(&str, &[(String, String)], &serde_json::Value) -> Result<String, String>,
    >(
        provider: &ProviderAdapter,
        goal_title: &str,
        goal_desc: Option<&str>,
        target_date: Option<&str>,
        context: &str,
        execute: F,
        repos: &Repos,
        identity: Option<&Identity>,
    ) -> Result<(String, PlanResult), DispatchError> {
        let (url, headers, body) = provider.request(
            &super::prompt::tier1_system(),
            &super::prompt::tier1_user(goal_title, goal_desc, target_date, context),
        );
        let raw = execute(&url, &headers, &body).map_err(DispatchError::BadJson)?;
        let resp: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| DispatchError::BadJson(e.to_string()))?;
        let text = provider
            .extract_text(&resp)
            .ok_or_else(|| DispatchError::BadJson("no content in response".into()))?;
        let plan = parse_plan(&text)?;
        let goal_id = persist_plan(
            repos,
            Some((goal_title, goal_desc, target_date)),
            &plan,
            identity,
        )?;
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
        let user_prompt = super::prompt::tier2_user(repos, today, constraints, velocity_json)?;
        let (url, headers, body) = provider.request(&super::prompt::tier2_system(), &user_prompt);
        let raw = execute(&url, &headers, &body).map_err(DispatchError::BadJson)?;
        let resp: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| DispatchError::BadJson(e.to_string()))?;
        let text = provider
            .extract_text(&resp)
            .ok_or_else(|| DispatchError::BadJson("no content in response".into()))?;
        let brief = parse_briefing(&text)?;
        persist_briefing(repos, &brief, today, identity)?;
        Ok(brief)
    }
}
