//! wl-sync: offline-first sync engine (Phase 8).
//!
//! Drains the CRDT outbox to the Axum relay, pulls remote ops,
//! decrypts and applies them locally with deterministic merge (US-4).

pub mod sync;

/// `wl-core` bounds a sealed op with a hand-mirrored literal so it can
/// stay free of `wl-protocol`; the relay rejects anything larger than
/// `wl_protocol::MAX_SEALED_BYTES`. If the mirror ever drifts LOW,
/// `wl-core` writes an op the relay will refuse, and the outbox can never
/// drain — sync wedges silently, with no test failing anywhere. This
/// crate depends on both sides, so the two numbers meet here at compile
/// time instead of in a comment nobody rereads.
const _: () = assert!(
    wl_core::store::repo::MAX_SEALED_OP_BYTES == wl_protocol::MAX_SEALED_BYTES,
    "wl-core's sealed-op cap has drifted from wl-protocol's: an op wl-core \
     accepts but the relay rejects can never drain from the outbox"
);
