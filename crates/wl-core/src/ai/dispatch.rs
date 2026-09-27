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
    /// The response arrived but was not the JSON we asked for.
    ///
    /// Kept strictly separate from [`DispatchError::Transport`], which is
    /// the fix for the bug that shipped: a request that timed out or never
    /// reached the provider was reported through this variant, so a network
    /// failure reached the user as *"response was not valid JSON: error
    /// sending request for url (…)"*. That names a parsing problem where
    /// the actual one was a network, which is the opposite of actionable.
    #[error("the architect's reply was not usable JSON: {0}")]
    BadJson(String),
    /// The HTTP hop itself failed: DNS, TLS, connect, timeout, body read.
    ///
    /// `#[source]`-free on purpose — the inner string is already
    /// reqwest's own wording and duplicating it would print twice.
    #[error("could not reach {provider}: {detail}")]
    Transport { provider: String, detail: String },
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

/// Ceiling on the architect's completion, in tokens.
///
/// The request used to send none, which made the response size bounded only
/// by the provider's own default (often 8k) and left an unbounded
/// completion free to run for minutes — the mechanism behind the button
/// that said "Planning…" for two minutes and then failed. A valid plan is
/// 1–5 milestones with a handful of directives and short phases; 4096
/// tokens is roughly twice what the schema's maximum plausible plan needs,
/// so a model that would overrun it was going to produce something the
/// validator rejects anyway.
pub const MAX_COMPLETION_TOKENS: u32 = 4096;

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
                    // Bounded, always. See the constant's doc.
                    "max_tokens": MAX_COMPLETION_TOKENS,
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

    /// The provider id this adapter talks to, for error messages.
    ///
    /// Derived from the base URL rather than stored, because the adapter
    /// never carried a provider id and adding one means every construction
    /// site in three crates grows a field for the sake of a string that is
    /// already in the URL.
    pub fn provider_label(&self) -> &'static str {
        match self {
            ProviderAdapter::OpenAiCompat { base_url, .. } => {
                if base_url.contains("openrouter") {
                    "openrouter"
                } else if base_url.contains("googleapis") {
                    "google"
                } else if base_url.contains("bytez") {
                    "bytez.com"
                } else {
                    "the provider"
                }
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
// Structured plan outputs (the architect's draft shape)
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
    /// The index — within this plan's directives, counted in reading order
    /// across all milestones, from 0 — of the directive that must be
    /// finished first.
    ///
    /// An **index**, not an id, and the reason is that the model has no way
    /// to know our id scheme: `new_id("dir")` mints a uuid at write time.
    /// Asking for an id would get a hallucinated string back, which reads
    /// exactly like a real edge until you follow it. An index is checkable,
    /// which is what makes the edge worth having at all.
    #[serde(default)]
    pub after: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MilestoneDraft {
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub directives: Vec<DirectiveDraft>,
    /// One sentence on why this milestone sits where it does.
    ///
    /// Rendered as the plan preview's "Why this order" line. A stated
    /// reason, not a chain of thought: the preview's job is to let a person
    /// disagree with the sequencing, and a 2 000-token derivation is the
    /// fastest way to make them skip it.
    #[serde(default)]
    pub rationale: Option<String>,
}

/// Full Tier-1 result: the goal the architect named, plus its milestone
/// hierarchy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanResult {
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

/// A fetched, repaired plan, and whether it is a real one.
///
/// The shape the compose screen's Generate button gets back. It is
/// **not** persisted: the user sees it as a dependency graph, edits it, and
/// only then calls `commit_plan`. A `fallback` preview still commits to the
/// same screen, with one editable task in it, so "the architect did not
/// come back with anything" never becomes "you have no goal".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlanPreview {
    pub plan: PlanResult,
    #[serde(default)]
    pub repair: RepairReport,
    /// The *Estimated Complexity* rating this plan was built for.
    ///
    /// Carried in the preview rather than re-sent by `commit_plan` for one
    /// reason: a rating that has to survive two IPC calls as two separate
    /// `Option`/int parameters is a rating that can be silently defaulted,
    /// and a goal written at 3/5 when the user chose 5/5 poisons the
    /// estimator permanently. Inside the payload there is nowhere for it to
    /// go missing.
    pub complexity: i64,
    /// `Some(reason)` when the response was unusable and this is the seeded
    /// one-task stand-in. The reason is shown to the user; a silent
    /// downgrade would show them a one-task plan and let them believe the
    /// architect produced it.
    #[serde(default)]
    pub fallback: Option<String>,
}

impl PlanPreview {
    /// The one-task stand-in, built from the user's own words.
    ///
    /// Seeded the way the manual path seeds: a 25-minute first step titled
    /// from the goal, on today's date, so the canvas has something runnable
    /// the moment the plan is committed. 25 stays under the progressive
    /// threshold, so there are no phase rows to author.
    pub fn fallback(intent: &str, target_date: Option<&str>, complexity: i64) -> PlanPreview {
        let title = fallback_title(intent);
        PlanPreview {
            plan: PlanResult {
                title: Some(title.clone()),
                description: None,
                milestones: vec![MilestoneDraft {
                    title: "First steps".into(),
                    description: None,
                    rationale: None,
                    directives: vec![DirectiveDraft {
                        title,
                        execution_context: None,
                        // The rating reaches the fallback too, but it does
                        // not change the length of a first sitting: a 25
                        // minute task is 25 minutes at any complexity.
                        estimated_minutes: 25,
                        phases: Vec::new(),
                        after: None,
                    }],
                }],
            },
            repair: RepairReport::default(),
            complexity,
            fallback: Some(format!(
                "The architect's plan could not be used, so this is a first step \
                 from your own words instead{}.",
                target_date
                    .map(|d| format!(" (target {d})"))
                    .unwrap_or_default()
            )),
        }
    }

    /// Whether this is the seeded stand-in.
    pub fn is_fallback(&self) -> bool {
        self.fallback.is_some()
    }
}

impl PlanResult {
    /// How many directives the plan holds, in reading order.
    ///
    /// The same order `persist_plan` mints ids in, so a draft's `after`
    /// index and the collected id list agree by construction rather than by
    /// two traversals that have to be kept in step.
    pub fn directive_count(&self) -> usize {
        self.milestones.iter().map(|m| m.directives.len()).sum()
    }

    /// The `index`-th directive's title, flattened the same way.
    ///
    /// Returns `None` past the end rather than panicking: the only caller
    /// is the warning path for a malformed edge, and a malformed edge is
    /// exactly the case that must not be able to take the process down.
    pub fn directive_title(&self, index: usize) -> Option<&str> {
        let mut left = index;
        for m in &self.milestones {
            if left < m.directives.len() {
                return Some(m.directives[left].title.as_str());
            }
            left -= m.directives.len();
        }
        None
    }

    /// Every directive, flattened, in reading order.
    pub fn directives(&self) -> impl Iterator<Item = &DirectiveDraft> {
        self.milestones.iter().flat_map(|m| m.directives.iter())
    }

    /// The same order, mutably — `repair_plan`'s edge pass.
    pub fn directives_mut(&mut self) -> impl Iterator<Item = &mut DirectiveDraft> {
        self.milestones
            .iter_mut()
            .flat_map(|m| m.directives.iter_mut())
    }
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
/// Ceiling on milestones, shared by `validate_plan` and `repair_plan`.
///
/// They were separate literals once, and the repair pass truncating to a
/// different number than the validator accepted is precisely how a
/// "repaired" plan could still be rejected. One constant, two readers.
pub const MAX_MILESTONES: usize = 5;

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

/// Deserializes a reply into a plan, WITHOUT judging its shape.
///
/// Parsing and validating are separate questions, and the order matters
/// more than it looks. This used to call [`validate_plan`] itself, which
/// meant a plan the repair pass exists to fix was rejected before the
/// repair pass could see it — the whole 2026-09-27 pass was dead code
/// behind one line. The tier path is now
/// `parse_plan` → [`repair_plan`] → [`validate_plan`], and the last of
/// those is still run before anything is written.
///
/// The only things that can still fail here are the ones no amount of
/// repair can address: no JSON object in the reply, or a field of the wrong
/// type entirely.
pub fn parse_plan(raw: &str) -> Result<PlanResult, DispatchError> {
    validate_response_size(raw)?;
    let cleaned = strip_fences(raw);
    let block = extract_json_block(cleaned)
        .ok_or_else(|| DispatchError::BadJson("no JSON object found".into()))?;
    serde_json::from_str(block).map_err(|e| DispatchError::BadJson(e.to_string()))
}

/// Parses and repairs in one step — the tier path's entry point.
///
/// Returns the report so the caller can tell the user what it changed
/// without a second pass over the plan to work it out.
pub fn parse_and_repair(
    raw: &str,
    intent: &str,
) -> Result<(PlanResult, RepairReport), DispatchError> {
    let mut plan = parse_plan(raw)?;
    let report = repair_plan(&mut plan, intent);
    Ok((plan, report))
}

/// The schema contract a plan has to meet before it can be written.
///
/// `pub` because it is the acceptance criterion for
/// [`repair_plan`]: "repaired" means exactly "this now returns `Ok`", and a
/// test that could not call it would have to approximate that. The tier
/// path still runs it before persisting, so a repair pass that misses a
/// case costs a rejected plan rather than a malformed row.
pub fn validate_plan(p: &PlanResult) -> Result<(), DispatchError> {
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

/// What `persist_plan` wrote, and what it had to leave out.
///
/// The warnings are not diagnostics — they are the only record of a plan
/// that was *not* written as the architect drew it. An edge the model got
/// wrong is dropped rather than failing the request (a plan with no ordering
/// constraint is still a plan), and dropping it silently would show the
/// user a preview that disagrees with what was committed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PersistedPlan {
    pub goal_id: String,
    /// Human-readable notes, in the order they were found.
    pub warnings: Vec<String>,
}

/// Persists a Tier-1 plan: goal, milestones, directives, and the
/// dependency edges between them.
///
/// `goal_spec`: `(fallback_title, description, target_date)` — a new
/// goal is created from it. (Restructuring an existing goal re-uses this
/// with the goal closed and re-planned under a fresh goal.)
///
/// The whole [`PlanPreview`] rather than `(plan, complexity)`, because
/// the rating has to reach `create_goal` and the only way to guarantee that
/// is to not ask the caller to pass it separately. A rating dropped between
/// the fetch and the write is a rating that silently becomes 3, and the
/// estimator then learns from a bucket the user never chose.
///
/// Title precedence: the architect's own `plan.title` wins, because the
/// user gave it free text rather than a name and naming the goal is the
/// model's job. `goal_spec`'s title is the fallback for a response that
/// omitted one, and `"Untitled goal"` is the last resort so a goal is
/// never created nameless.
///
/// **Two passes over the directives**, and the reason is the index-based
/// edge: a draft names its prerequisite by position, but the id it will be
/// stored under does not exist until the row is written. So every
/// directive is created first, collecting ids in reading order, and only
/// then are the edges linked. A single pass would have to guess ids.
pub fn persist_plan(
    repos: &Repos,
    goal_spec: Option<(&str, Option<&str>, Option<&str>)>,
    preview: &PlanPreview,
    identity: Option<&Identity>,
) -> Result<PersistedPlan, DispatchError> {
    let plan = &preview.plan;
    validate_plan(plan)?;
    let mut warnings: Vec<String> = Vec::new();
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
                "Untitled goal".into()
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
        .create_goal(
            &title,
            desc.as_deref(),
            target.as_deref(),
            preview.complexity,
            identity,
        )?
        .id;
    // CORE-3: a new plan supersedes the previous one. Without this every
    // `persist_plan` left the old goal `active` forever, so goals
    // accumulated in the `active` state and `active_goal()`'s `LIMIT 1`
    // became an arbitrary tie-break. Runs AFTER the new goal exists so
    // `keep` can never archive the goal we just created.
    repos.archive_other_active_goals(&goal_id, identity)?;
    // Pass 1: every directive, in reading order, collecting its new id so
    // the `after` indices in pass 2 can be resolved to real ids.
    let mut created: Vec<String> = Vec::with_capacity(plan.directive_count());
    let mut edges: Vec<(usize, usize)> = Vec::new();
    for (i, m) in plan.milestones.iter().enumerate() {
        let ms = repos.create_milestone(
            &goal_id,
            &m.title,
            m.description.as_deref(),
            i as i64,
            identity,
        )?;
        for d in &m.directives {
            let index = created.len();
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
            let row = repos.create_directive(
                &ms.id,
                &d.title,
                d.execution_context.as_deref(),
                mins,
                total,
                &crate::domain::today_local(),
                None,
                &phases,
                identity,
            )?;
            created.push(row.id);
            if let Some(after) = d.after {
                edges.push((index, after));
            }
        }
    }
    // Pass 2: the edges. Anything unusable is dropped and reported, never
    // fatal — a plan the user can still run beats a plan that is refused
    // over one bad index. `repair_plan` has already cleared the forward and
    // self references, so this is the second gate rather than the first.
    for (index, after) in edges {
        let title = plan.directive_title(index).unwrap_or("a task");
        if after >= index {
            warnings.push(format!(
                "“{title}” points at a task that comes after it; that link was dropped"
            ));
            continue;
        }
        if let Err(e) =
            repos.set_directive_depends_on(&created[index], Some(&created[after]), identity)
        {
            warnings.push(format!(
                "“{title}” could not be linked to its prerequisite: {e}"
            ));
        }
    }
    warnings.extend(preview.repair.notes.iter().cloned());
    Ok(PersistedPlan { goal_id, warnings })
}

// ---------------------------------------------------------------------------
// Repair — make an almost-right plan right
// ---------------------------------------------------------------------------

/// What repair had to change, in the order it found it.
///
/// Returned to the UI, not logged. A plan that was quietly reshaped is a
/// plan the user approved a preview of, and the preview is the thing they
/// are looking at when they decide whether to trust it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RepairReport {
    #[serde(default)]
    pub notes: Vec<String>,
}

impl RepairReport {
    fn note(&mut self, msg: String) {
        self.notes.push(msg);
    }

    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
    }
}

/// Brings a parsed plan inside the store's write boundary, in place.
///
/// The shape this replaces was **all-or-nothing**: `validate_plan` ran over
/// the whole response and any single violation — one directive over 30
/// minutes with no phases, a sixth milestone, a phase sum outside the 2×
/// band — rejected the entire plan, so a user who paid for a BYOK call got
/// no goal at all. The user's report was that the AI "doesn't make any task
/// for me", and this is the code behind that.
///
/// So: clamp what can be clamped, synthesise what can be synthesised, and
/// drop only what has to go. The only thing that still fails the whole
/// response is having no usable directive at all, and the caller turns even
/// that into a seeded first step rather than an error.
///
/// Deterministic, and a pure function of its input apart from the report —
/// a plan repaired twice from the same JSON is the same plan.
pub fn repair_plan(plan: &mut PlanResult, intent: &str) -> RepairReport {
    let mut report = RepairReport::default();

    // Title: a model that named nothing gets the user's own words, and an
    // over-long one is clipped rather than rejected.
    match plan.title.as_deref().map(str::trim) {
        Some(t) if !t.is_empty() => {
            if let Some(clipped) = clip(t, crate::domain::MAX_TITLE_CHARS) {
                if clipped != t {
                    report.note(format!(
                        "The goal name was too long and was shortened to “{clipped}”."
                    ));
                    plan.title = Some(clipped.to_string());
                }
            }
        }
        _ => {
            plan.title = Some(fallback_title(intent));
            report.note("The architect did not name the goal; your own words were used.".into());
        }
    }
    if let Some(d) = plan.description.as_deref() {
        if let Some(clipped) = clip(d, crate::domain::MAX_DESCRIPTION_CHARS) {
            if clipped != d {
                report.note("The goal description was shortened to fit.".into());
                plan.description = Some(clipped.to_string());
            }
        }
    }

    // Milestones: cap the count, then drop the empty ones — but never drop
    // the last one, so "a plan with nothing in it" stays an error rather
    // than becoming an empty goal.
    if plan.milestones.len() > MAX_MILESTONES {
        report.note(format!(
            "Only the first {MAX_MILESTONES} milestones were kept; the rest were cut."
        ));
        plan.milestones.truncate(MAX_MILESTONES);
    }
    for m in plan.milestones.iter_mut() {
        if let Some(clipped) = clip(&m.title, crate::domain::MAX_TITLE_CHARS) {
            if clipped != m.title {
                m.title = clipped.to_string();
            }
        }
        m.title = repair_text(m.title.trim(), "Milestone", &mut report);
    }
    for m in plan.milestones.iter_mut() {
        if let Some(clipped) = clip(
            m.description.as_deref().unwrap_or(""),
            crate::domain::MAX_DESCRIPTION_CHARS,
        ) {
            if clipped != m.description.as_deref().unwrap_or("") {
                m.description = Some(clipped.to_string());
            }
        }
    }

    // Directives, inside each milestone.
    for m in plan.milestones.iter_mut() {
        if m.directives.len() > MAX_DIRECTIVES_PER_MILESTONE {
            report.note(format!(
                "“{}” listed more than {MAX_DIRECTIVES_PER_MILESTONE} tasks; the extras were cut.",
                m.title
            ));
            m.directives.truncate(MAX_DIRECTIVES_PER_MILESTONE);
        }
        for d in m.directives.iter_mut() {
            repair_directive(d, &mut report);
        }
    }
    // A milestone left with nothing in it is noise, and dropping it is safe
    // as long as one survives — which the `directive_count` check in the
    // caller enforces. One collapsed note rather than one per drop: five
    // identical lines on a page whose whole job is to be scannable.
    if plan.milestones.len() > 1 {
        let before = plan.milestones.len();
        plan.milestones.retain(|m| !m.directives.is_empty());
        if plan.milestones.len() != before {
            report.note(format!(
                "{} milestone(s) with no usable tasks were dropped.",
                before - plan.milestones.len()
            ));
        }
    }

    // Edges: a forward or self reference is not a plan defect, it is one
    // bad index. `persist_plan` drops it and reports; clearing it here means
    // the PREVIEW shows what will actually be written, which is the entire
    // point of showing a preview.
    let total = plan.directive_count();
    for (i, d) in plan.directives_mut().enumerate() {
        if let Some(after) = d.after {
            if after >= i || after >= total {
                d.after = None;
            }
        }
    }
    report
}

/// Clamps one directive's numbers and titles, or drops it if it has no
/// usable identity left.
fn repair_directive(d: &mut DirectiveDraft, report: &mut RepairReport) {
    d.title = repair_text(d.title.trim(), "A task", report);
    if let Some(clipped) = clip(
        d.execution_context.as_deref().unwrap_or(""),
        crate::domain::MAX_CONTEXT_CHARS,
    ) {
        if clipped != d.execution_context.as_deref().unwrap_or("") {
            d.execution_context = Some(clipped.to_string());
        }
    }
    // A nonpositive or absurd estimate is clamped rather than rejected: the
    // store's bound is 1..=1440 and any value outside it is a model
    // miscount, not a statement about the work.
    if d.estimated_minutes <= 0 {
        d.estimated_minutes = Directive::PROGRESSIVE_THRESHOLD_MINUTES;
        report.note(format!(
            "“{}” had no time estimate; {} minutes was assumed.",
            d.title, d.estimated_minutes
        ));
    } else if d.estimated_minutes > crate::domain::MAX_MINUTES {
        d.estimated_minutes = crate::domain::MAX_MINUTES;
        report.note(format!(
            "“{}” claimed more than a day; the estimate was capped.",
            d.title
        ));
    }
    // Phases: drop the unusable ones, then reconcile the estimate with what
    // survived. The old validator rejected the whole plan when the sum fell
    // outside a 2× band around the estimate; reconciling instead is the
    // whole point, and the phase table WINS because it is what the user
    // will actually be shown.
    let before = d.phases.len();
    d.phases
        .retain(|p| p.minutes > 0 && p.minutes <= crate::domain::MAX_MINUTES);
    if d.phases.len() != before {
        report.note(format!(
            "“{}” had phases with impossible times; {} were dropped.",
            d.title,
            before - d.phases.len()
        ));
    }
    for p in d.phases.iter_mut() {
        if let Some(clipped) = clip(&p.title, crate::domain::MAX_TITLE_CHARS) {
            p.title = clipped.to_string();
        }
        if p.title.trim().is_empty() {
            p.title = "Continue".into();
        }
    }
    if d.phases.is_empty() {
        if d.estimated_minutes > Directive::PROGRESSIVE_THRESHOLD_MINUTES {
            // The one synthesis: a long task with no phases cannot be stored
            // (`create_directive` requires progressive rows above the
            // threshold), and the old code REJECTED the plan for it. Split
            // it deterministically instead.
            let (phases, total) = synthesise_phases(d.estimated_minutes);
            if total < d.estimated_minutes {
                report.note(format!(
                    "“{}” is longer than {MAX_SYNTHESIS_PHASES} steps of {PHASE_MAX} \
                     minutes can cover; it was capped at {total} minutes. Consider making \
                     it several tasks.",
                    d.title
                ));
            } else {
                report.note(format!(
                    "“{}” had no steps for a {total} minute task; it was split into {}.",
                    d.title,
                    phases.len()
                ));
            }
            d.phases = phases;
            d.estimated_minutes = total;
        }
    } else {
        let title = d.title.clone();
        let total = reconcile_phases(&mut d.phases, d.estimated_minutes, &title, report);
        d.estimated_minutes = total;
    }
}

/// The 5–25 minute band a phase is supposed to sit in (PRD §5.2).
const PHASE_MIN: i64 = 1;
const PHASE_MAX: i64 = 25;

/// Most steps a synthesised task is split into — and the schema's own
/// ceiling, which `validate_directive` enforces as `2..=4`.
///
/// This is a real limit rather than a preference, and it has a consequence
/// worth stating: four steps of at most 25 minutes is 100 minutes, so a
/// directive the model estimated at four hours cannot be represented as
/// four phases. The old code rejected the entire plan for that. Now the
/// task is capped at 100 minutes and the note tells the user it should
/// have been several tasks — which is the actual advice, and the same
/// thing the old bailout modal said when it split a scope downsize.
const MAX_SYNTHESIS_PHASES: i64 = 4;

/// Steps needed to hold `minutes` inside the per-phase band.
fn phase_count_for(minutes: i64) -> i64 {
    // Hand-rolled rather than `div_ceil`, which is still unstable on this
    // toolchain (rust-lang#88581).
    ((minutes + PHASE_MAX - 1) / PHASE_MAX).clamp(2, MAX_SYNTHESIS_PHASES)
}

/// Splits `estimate` into steps that all sit inside the band, returning the
/// steps and the total they actually cover.
///
/// The total is **not** always the estimate: `count × 25` is a hard ceiling
/// and a longer task is capped rather than split into out-of-band steps.
/// That is why the caller compares the two and reports the difference.
fn synthesise_phases(estimate: i64) -> (Vec<PhaseDraft>, i64) {
    let count = phase_count_for(estimate);
    let total = estimate.min(count * PHASE_MAX);
    (even_phases(count, total), total)
}

/// `count` phases summing exactly to `total`, as even as possible.
fn even_phases(count: i64, total: i64) -> Vec<PhaseDraft> {
    let count = count.max(1);
    let each = (total / count).clamp(PHASE_MIN, PHASE_MAX);
    (0..count)
        .map(|i| {
            // The last absorbs the remainder so the sum is exact.
            let minutes = if i == count - 1 {
                (total - each * (count - 1)).clamp(PHASE_MIN, PHASE_MAX)
            } else {
                each
            };
            PhaseDraft {
                title: format!("Step {}", i + 1),
                instruction: None,
                minutes,
            }
        })
        .collect()
}

/// Brings a model-supplied phase table inside the band, and returns the
/// total the steps now add up to.
///
/// Three operations, in this order, and the order is the point:
/// distribute the estimate, then split anything still over 25, then clamp.
/// Clamping first would silently eat time; distributing into more steps
/// does not. And the returned total **becomes** the directive's estimate —
/// `persist_plan` already lets the phase sum override the headline number,
/// so making them agree here means the preview, the badge and the stored
/// row cannot disagree.
fn reconcile_phases(
    phases: &mut Vec<PhaseDraft>,
    estimate: i64,
    title: &str,
    report: &mut RepairReport,
) -> i64 {
    let before = phases.len();
    let old_total: i64 = phases.iter().map(|p| p.minutes).sum();
    if old_total <= 0 || estimate < phases.len() as i64 {
        return old_total;
    }
    if old_total != estimate {
        distribute(phases, estimate);
        report.note(format!(
            "“{title}” phases were rescaled to its {estimate} minute estimate."
        ));
    }
    // Split any step still over the band, longest first, until nothing
    // exceeds it or we run out of steps.
    while let Some(widest) = phases
        .iter()
        .enumerate()
        .filter(|(_, p)| p.minutes > PHASE_MAX)
        .max_by_key(|(_, p)| p.minutes)
        .map(|(i, _)| i)
    {
        if phases.len() as i64 >= MAX_SYNTHESIS_PHASES {
            break;
        }
        let half = phases[widest].minutes / 2;
        let rest = phases[widest].minutes - half;
        let at = widest + 1;
        phases.insert(
            at,
            PhaseDraft {
                title: format!("{} (cont.)", phases[widest].title),
                instruction: None,
                minutes: half.max(PHASE_MIN),
            },
        );
        phases[widest].minutes = rest.max(PHASE_MIN);
    }
    // Anything still over the band is a task longer than eight steps can
    // honestly cover. Clamp it, and say so, because the number the user
    // approved just changed.
    let mut clamped = false;
    for p in phases.iter_mut() {
        if p.minutes > PHASE_MAX {
            p.minutes = PHASE_MAX;
            clamped = true;
        }
    }
    let total: i64 = phases.iter().map(|p| p.minutes).sum();
    if clamped {
        report.note(format!(
            "“{title}” is longer than {MAX_SYNTHESIS_PHASES} steps allow; \
             it was capped at {total} minutes. Consider splitting it into two tasks."
        ));
    }
    if phases.len() != before {
        report.note(format!(
            "“{title}” was split into {} steps to keep each under {PHASE_MAX} minutes.",
            phases.len()
        ));
    }
    total
}

/// Redistributes `total` across `phases`, keeping each phase's share of the
/// original and letting the last absorb the rounding.
fn distribute(phases: &mut [PhaseDraft], total: i64) {
    let old: i64 = phases.iter().map(|p| p.minutes).sum();
    if old <= 0 || total < phases.len() as i64 {
        return;
    }
    let mut remaining = total;
    let last = phases.len() - 1;
    for (i, p) in phases.iter_mut().enumerate() {
        p.minutes = if i == last {
            remaining
        } else {
            let scaled =
                (i128::from(p.minutes) * i128::from(total) + i128::from(old) / 2) / i128::from(old);
            // One minute per remaining phase is the floor; anything less
            // would make `create_directive` reject the whole row. The i128
            // arithmetic is the same widening the store's own rescale uses,
            // for the same reason: `total` is bounded by MAX_MINUTES but the
            // product is not obviously so.
            let headroom = (remaining - (last - i) as i64).max(PHASE_MIN) as i128;
            scaled.clamp(PHASE_MIN as i128, headroom) as i64
        };
        remaining -= p.minutes;
    }
}

/// A display name for something the model left blank.
///
/// `fallback` exists because a blank milestone title and a blank task title
/// want different words, and a generic "Untitled" on every row of a
/// five-milestone plan is a list the user cannot tell apart.
fn repair_text(text: &str, fallback: &str, report: &mut RepairReport) -> String {
    if !text.is_empty() {
        return text.to_string();
    }
    report.note(format!(
        "{fallback} had no name and was labelled “{fallback}”."
    ));
    fallback.to_string()
}

/// Clip to `max` characters, preferring a word boundary. `None` when the
/// text already fits.
///
/// Owned rather than borrowed: every call site assigns the result back into
/// the field being repaired, and a `&str` return would be a borrow of a
/// local.
fn clip(text: &str, max: usize) -> Option<String> {
    if text.chars().count() <= max {
        return None;
    }
    let clipped: String = text.chars().take(max).collect();
    Some(match clipped.rfind(' ') {
        Some(i) if i > 0 => clipped[..i].trim_end().to_string(),
        _ => clipped,
    })
}

// ---------------------------------------------------------------------------
// Dispatcher facade
// ---------------------------------------------------------------------------

/// Orchestrates a tier call: build prompt → (shell executes HTTP) →
/// parse → persist. The HTTP hop is injected so this type stays
/// platform-clean and unit-testable.
pub struct AiDispatcher;

impl AiDispatcher {
    /// Tier 1 — fetch a plan and return it **without writing anything**.
    ///
    /// This is the billable half. The compose screen's Generate button
    /// awaits this, shows a stage line while it runs, and lands on the plan
    /// preview — and the user has not yet created a goal, a milestone or a
    /// task. Backing out of the preview costs nothing because there is
    /// nothing to back out of, which is the whole reason the call was split
    /// in two: the old shape persisted the plan inside the request that
    /// fetched it, so "let me look at it first" was not expressible.
    ///
    /// `complexity` is the user's *Estimated Complexity* rating, 1–5, and
    /// `record` is the calibration estimator's read of the same bucket, so
    /// the prompt can say what the rating has historically been worth
    /// rather than treating opinion as fact.
    ///
    /// Validated before the call, not after: a blank intent or a bad rating
    /// should not cost a billable request.
    #[allow(clippy::too_many_arguments)] // explicit params keep the injected-HTTP design testable.
    pub fn master_plan_preview<F>(
        provider: &ProviderAdapter,
        intent: &str,
        target_date: Option<&str>,
        complexity: i64,
        record: Option<&str>,
        execute: F,
    ) -> Result<PlanPreview, DispatchError>
    where
        F: Fn(&str, &[(String, String)], &serde_json::Value) -> Result<String, String>,
    {
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
        crate::domain::check_complexity("complexity", complexity).map_err(|why| {
            DispatchError::Invalid {
                field: "complexity",
                why,
            }
        })?;
        let (url, headers, body) = provider.request(
            &super::prompt::tier1_system(),
            &super::prompt::tier1_user(intent, target_date, complexity, record),
        );
        // A transport failure is its own error now. This is the change that
        // makes "Planning…" diagnosable: a timeout used to arrive as
        // "response was not valid JSON: error sending request…", which
        // blames the reply for a network problem.
        let raw = execute(&url, &headers, &body).map_err(|detail| DispatchError::Transport {
            provider: provider.provider_label().to_string(),
            detail,
        })?;
        validate_response_size(&raw)?;
        let resp: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| DispatchError::BadJson(e.to_string()))?;
        let text = provider
            .extract_text(&resp)
            .ok_or_else(|| DispatchError::BadJson("no content in response".into()))?;
        let (plan, repair) = super::dispatch::parse_and_repair(&text, intent)?;
        // A plan with nothing in it cannot be previewed, and the honest
        // answer to that is the seeded fallback rather than an error the
        // user cannot act on.
        if plan.directive_count() == 0 {
            return Ok(PlanPreview::fallback(intent, target_date, complexity));
        }
        // Belt and braces: repair is meant to make this unreachable, and if
        // it ever is not, the plan is still better than nothing.
        super::dispatch::validate_plan(&plan)?;
        Ok(PlanPreview {
            plan,
            repair,
            complexity,
            fallback: None,
        })
    }

    /// Tier 1, the old shape: fetch and persist in one call.
    ///
    /// Kept because the engine tests and the injected-HTTP test suite
    /// exercise persistence through it, and because it is the honest
    /// composition of the two halves. Nothing in the UI calls it: the
    /// compose screen goes preview → commit, because a plan the user has
    /// not seen must not be written.
    #[allow(clippy::too_many_arguments)]
    pub fn master_plan<F>(
        provider: &ProviderAdapter,
        intent: &str,
        target_date: Option<&str>,
        complexity: i64,
        record: Option<&str>,
        execute: F,
        repos: &Repos,
        identity: Option<&Identity>,
    ) -> Result<(PersistedPlan, PlanResult), DispatchError>
    where
        F: Fn(&str, &[(String, String)], &serde_json::Value) -> Result<String, String>,
    {
        let preview =
            Self::master_plan_preview(provider, intent, target_date, complexity, record, execute)?;
        // `intent` doubles as the title fallback: if the model named the
        // goal, this is ignored.
        let persisted = persist_plan(repos, Some((intent, None, target_date)), &preview, identity)?;
        Ok((persisted, preview.plan))
    }
}
