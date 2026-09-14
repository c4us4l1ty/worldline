//! wl-core: Worldline's pure-Rust local-first core.
//!
//! Platform-clean (no tokio, no Tauri deps) so the same crate powers
//! desktop, mobile, and headless integration tests. All secrets flow
//! through [`crypto`] and are zeroized on drop.

pub mod ai;
pub mod crdt;
pub mod crypto;
pub mod domain;
pub mod engine;
pub mod hlc;
pub mod store;

pub use crypto::identity::{Identity, IdentityVault};
