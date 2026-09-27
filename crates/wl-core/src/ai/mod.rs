//! BYOK AI engine (PRD §4): 3-tier hierarchical dispatcher.
//!
//! Tier 1 — Master Architect (goal creation / restructuring).
//! Tier 2 — Tactical Dispatcher (morning briefing, 1–3 directives).
//! Tier 3 — Local Heuristic Engine (see [`crate::engine`], offline).
//!
//! Provider adapters: OpenAI-compatible chat completions for the three
//! approved BYOK providers (OpenRouter, Google, bytez.com).
//! All model ids are user-configurable BYOK settings — never hardcoded,
//! and never restricted to a list baked into this binary: [`catalog`]
//! discovers whatever the provider currently serves, so a model
//! released upstream is selectable without a Worldline release.

pub mod catalog;
pub mod dispatch;
pub mod prompt;

#[cfg(test)]
mod tests;

pub use dispatch::{AiDispatcher, BriefingResult, DispatchError, PlanResult};
