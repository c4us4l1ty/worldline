//! BYOK AI engine (PRD §4).
//!
//! Tier 1 — Master Architect (goal creation / restructuring).
//! Tier 3 — Local Heuristic Engine (see [`crate::engine`], offline).
//!
//! The Tier-2 Tactical Dispatcher that sat between them was removed
//! 2026-09-27 with the morning-briefing screen (PRD delta 167): the
//! canvas is the directive, and a second AI tier that authored a day of
//! directives had no surface left to justify it.
//!
//! Provider adapters: OpenAI-compatible chat completions for the three
//! All model ids are user-configurable BYOK settings — never hardcoded,
//! and never restricted to a list baked into this binary: [`catalog`]
//! discovers whatever the provider currently serves, so a model
//! released upstream is selectable without a Worldline release.

pub mod catalog;
pub mod dispatch;
pub mod prompt;

#[cfg(test)]
mod tests;

pub use dispatch::{AiDispatcher, DispatchError, PlanResult};
