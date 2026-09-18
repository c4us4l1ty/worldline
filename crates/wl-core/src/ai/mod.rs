//! BYOK AI engine (PRD §4): 3-tier hierarchical dispatcher.
//!
//! Tier 1 — Master Architect (goal creation / restructuring).
//! Tier 2 — Tactical Dispatcher (morning briefing, 1–3 directives).
//! Tier 3 — Local Heuristic Engine (see [`crate::engine`], offline).
//!
//! Provider adapters: OpenAI-compatible chat completions for the four
//! approved BYOK providers (OpenRouter, Google, Qwen, bytez.com).
//! All model ids are user-configurable BYOK settings — never hardcoded.

pub mod dispatch;
pub mod prompt;

#[cfg(test)]
mod tests;

pub use dispatch::{AiDispatcher, BriefingResult, DispatchError, PlanResult};
