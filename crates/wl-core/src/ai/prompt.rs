//! Prompt builders for Tier 1 (Master Architect). Prompts are pure
//! functions of local state — no secrets, no personal identifiers
//! beyond what the user typed.

use super::dispatch::{DirectiveDraft, MilestoneDraft};

/// System prompt for Tier 1: names the goal and generates the milestone
/// hierarchy.
pub fn tier1_system() -> String {
    "You are the Master Architect of a Stackelberg productivity system.\
The user is the executor; you are the planner.\
The user supplies raw intent in their own words — it may be a fragment,\
ungrammatical, or contain constraints. You name the goal from it.\
Given a goal, produce a lean milestone hierarchy with a critical path and\
realistic task estimates.\
Respond ONLY with JSON matching the provided schema.\
Each directive must be a concrete action verb with a quantifiable constraint\
(e.g. 'Write 300 words on Section 2.1').\
Directives longer than 30 minutes must be split into progressive phases of\
5–25 minutes each to defeat procrastination."
        .into()
}

/// User prompt for Tier 1 goal structuring.
///
/// One free-text intent, not a title plus a description plus a
/// constraints field — the compose screen collects a single box, and
/// anything the user would have put in "details" or "constraints"
/// (hours per day, skills, hard deadlines) is in here instead. The
/// prompt says so explicitly, because a model that assumes a tidy
/// single-line GOAL field will otherwise read a paragraph as a title and
/// plan against the whole thing verbatim.
///
/// `complexity` is the user's own 1–5 *Estimated Complexity* rating and
/// `record` is the calibration estimator's read of that same bucket. Both
/// go in, and the reason `record` is here at all is that it is the
/// estimator's **only consumer**: without it the number computed by
/// `engine::calibration` would be displayed and never used, which is the
/// inert-figure failure this product does not ship.
pub fn tier1_user(
    intent: &str,
    target_date: Option<&str>,
    complexity: i64,
    record: Option<&str>,
) -> String {
    let mut s = format!(
        "USER INTENT (their own words; may be a fragment, a paragraph,\
or include constraints, hours/day, skills, and deadlines):\n{intent}\n"
    );
    if let Some(t) = target_date {
        s.push_str(&format!("TARGET DATE: {t}\n"));
    }
    if let Some(r) = record {
        s.push_str(&format!("{r}\n"));
    }
    // The rating's effect on the plan is spelled out rather than implied.
    // A model handed "difficulty: 4" with no guidance will either ignore
    // it or treat it as licence to produce 32 tasks; both are wrong.
    s.push_str(&format!(
        "SIZE THE PLAN TO THE {complexity}/5 CALIBRATION ABOVE: \
1–2 means one milestone and one or two short tasks; \
3 means two or three milestones of ordinary work; \
4–5 means three to five milestones, more conservative estimates, and \
phases broken out earlier.\n"
    ));
    let schema = "{\"title\":string,\"description\":string,\"milestones\":[{\"title\":string,\"description\":string,\"rationale\":string,\"directives\":[{\"title\":string,\"execution_context\":string,\"estimated_minutes\":number,\"after\":number,\"phases\":[{\"title\":string,\"instruction\":string,\"minutes\":number}]}]}]}";
    s.push_str(&format!(
        "\nProduce a milestone plan. JSON schema:\n{schema}\n\
title: name the goal yourself — short, imperative, under 60 characters.\
Read it as an intent, not as a title to echo back.\
description: optional one-sentence framing; may be empty.\
rationale: ONE short sentence on why this milestone sits where it does.\
Phases array: empty for directives <= 30 minutes, otherwise 2-4 phases\
totaling approximately estimated_minutes. 2-5 milestones. No prose outside JSON.\n\
after: the 0-based index of the directive that must be finished first, counted\
across the whole plan in reading order. Use it ONLY where one task genuinely\
cannot start before another. Leave it out for independent work — a plan with\
an edge for everything is a plan the user cannot see the shape of."
    ));
    s
}

/// Serializes milestone drafts for persistence logging (Tier 1 output).
pub fn milestones_to_json(drafts: &[MilestoneDraft]) -> String {
    serde_json::to_string(drafts).expect("draft ser")
}

/// Helper: phases for a draft directive (empty = monolithic).
pub fn phases_of(d: &DirectiveDraft) -> Vec<(String, Option<String>, i64)> {
    d.phases
        .iter()
        .map(|p| (p.title.clone(), p.instruction.clone(), p.minutes))
        .collect()
}
