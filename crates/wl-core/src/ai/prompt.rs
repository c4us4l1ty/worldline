//! Prompt builders for Tier 1 (Master Architect) and Tier 2 (Tactical
//! Dispatcher). Prompts are pure functions of local state — no
//! secrets, no personal identifiers beyond what the user typed.

use crate::store::repo::Repos;

use super::dispatch::{DirectiveDraft, MilestoneDraft};

/// System prompt for Tier 1: generates the milestone hierarchy.
pub fn tier1_system() -> String {
    "You are the Master Architect of a Stackelberg productivity system.\
The user is the executor; you are the planner.\
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
pub fn tier1_user(
    goal_title: &str,
    goal_description: Option<&str>,
    target_date: Option<&str>,
    context: &str,
) -> String {
    let mut s = format!("GOAL: {goal_title}\n");
    if let Some(d) = goal_description {
        s.push_str(&format!("DETAILS: {d}\n"));
    }
    if let Some(t) = target_date {
        s.push_str(&format!("TARGET DATE: {t}\n"));
    }
    let schema = "{\"milestones\":[{\"title\":string,\"description\":string,\"directives\":[{\"title\":string,\"execution_context\":string,\"estimated_minutes\":number,\"phases\":[{\"title\":string,\"instruction\":string,\"minutes\":number}]}]}]}";
    s.push_str(&format!(
        "USER CONTEXT (constraints, hours/day, skills): {context}\n\n\
Produce a milestone plan. JSON schema:\n{schema}\n\
Phases array: empty for directives <= 30 minutes, otherwise 2-4 phases\
totaling approximately estimated_minutes. 2-5 milestones. No prose outside JSON."
    ));
    s
}

/// System prompt for Tier 2: daily tactical dispatch.
pub fn tier2_system() -> String {
    "You are the Tactical Dispatcher of a Stackelberg productivity system.\
You receive the active milestone, recent velocity data, and the user's\
constraints for today.\
Emit 1–3 non-negotiable directives for TODAY ONLY.\
Each directive: action verb + quantifiable constraint, executable in one\
sitting. Never reference future days. Never produce lists of backlog.\
Respond ONLY with JSON matching the provided schema."
        .into()
}

/// User prompt for Tier 2, restricted to: active milestone + past 48h
/// check-in velocity + user constraints (PRD §4 context window rule).
pub fn tier2_user(
    repos: &Repos,
    today: &str,
    constraints: &str,
    velocity_json: &str,
) -> Result<String, crate::store::StoreError> {
    let mut s = String::new();
    if let Some(goal) = repos.active_goal()? {
        if let Some(m) = repos.next_pending_milestone(&goal.id)? {
            s.push_str(&format!("ACTIVE MILESTONE: {}\n", m.title));
            if let Some(d) = &m.description {
                s.push_str(&format!("MILESTONE DETAIL: {d}\n"));
            }
        }
    }
    s.push_str(&format!(
        "VELOCITY DATA (recent check-ins, objective): {velocity_json}\n"
    ));
    s.push_str(&format!("TODAY: {today}\n"));
    let schema = "{\"directives\":[{\"title\":string,\"execution_context\":string,\"estimated_minutes\":number,\"phases\":[{\"title\":string,\"instruction\":string,\"minutes\":number}]}]}";
    s.push_str(&format!(
        "USER CONSTRAINTS TODAY: {constraints}\n\n\
Produce directives for today. JSON schema:\n{schema}\n\
1-3 directives. Phases: empty if <= 30 minutes, else 2-4 progressive phases.\
No prose outside JSON."
    ));
    Ok(s)
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
