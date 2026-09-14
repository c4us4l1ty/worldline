//! wl-sync: offline-first sync engine (Phase 8).
//!
//! Drains the CRDT outbox to the Axum relay, pulls remote ops,
//! decrypts and applies them locally with deterministic merge (US-4).

pub mod sync;
