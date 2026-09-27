//! All Worldline screens: canvas, seed vault, check-in, goal creation,
//! the staged plan preview, dormant, the control panel's pages, the
//! drawers, and settings — each per the worldline skill.
//!
//! There is no BYOK screen: `Screen::ByokSetup` was unreachable from
//! every entry point, and the compose screen (`goal_create.rs`) now owns
//! the AI-or-manual choice while Settings owns the key itself. The
//! screen was deleted rather than left dormant.

mod canvas;
mod checkin;
mod dormant;
mod entropy_log;
mod goal_create;
mod model_picker;
mod nav_drawer;
mod plan_preview;
mod seed_vault;
mod settings;
mod telemetry;
mod trajectory;

pub use canvas::*;
pub use checkin::*;
pub use dormant::*;
pub use entropy_log::*;
pub use goal_create::*;
pub use model_picker::*;
pub use nav_drawer::*;
pub use plan_preview::*;
pub use seed_vault::*;
pub use settings::*;
pub use telemetry::*;
pub use trajectory::*;
