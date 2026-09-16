//! Axum relay — a blind, opaque message drop box (PRD §3).
//!
//! Routes:
//!   POST /auth/challenge {public_key}           → {nonce, expires_at}
//!   POST /auth/verify    {public_key, signature}→ {token, expires_at}
//!   POST /sync/push      {ops:[…]}              → {accepted:[…]}
//!   POST /sync/pull      {since_hlc, limit}     → {ops, next_cursor, exhausted}
//!
//! The relay validates Ed25519 signatures and routes ciphertext it can
//! never decrypt. Zero knowledge: no plaintext, no metadata beyond
//! routing headers ever persists.

pub mod auth;
pub mod store;

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use base64::Engine;
use serde_json::json;

use auth::{AuthError, AuthState};
use store::{BlobStore, StoredOp};

#[cfg(feature = "sqlite")]
use store::sqlite_backend::SqliteStore;

/// Hard cap per pull batch.
const PULL_HARD_CAP: u32 = 500;
/// Hard cap on ops per push request (storage-exhaustion + CPU bound).
const MAX_OPS_PER_PUSH: usize = 500;
/// Hard cap on one sealed blob (100× a normal op; sync payloads are KB).
const MAX_SEALED_BYTES: usize = 256 * 1024;
/// Routing-header length bound (ids are UUID-shaped; generous ceiling).
const MAX_HEADER_LEN: usize = 128;

pub struct AppState {
    pub auth: AuthState,
    pub blobs: Box<dyn BlobStore>,
}

pub type SharedState = Arc<AppState>;

pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/auth/challenge", post(challenge))
        .route("/auth/verify", post(verify))
        .route("/sync/push", post(push))
        .route("/sync/pull", post(pull))
        .with_state(state)
}

fn err(status: StatusCode, msg: &str) -> axum::response::Response {
    (status, Json(json!({"error": msg}))).into_response()
}

/// Runs a blocking store call off the async executor (the SQLite
/// backend holds a mutex + the Postgres backend drives its own
/// runtime — neither may run on an axum worker: the former would
/// stall the executor, the latter panics with "Cannot block the
/// current thread"). Store errors map to quota-429s or a generic 500:
/// raw storage strings never reach the wire (info-leak surface).
/// Callers move an `Arc` clone into `f` (it is `'static`).
#[allow(clippy::result_large_err)]
async fn blocking<F, T>(f: F) -> Result<T, axum::response::Response>
where
    F: FnOnce() -> Result<T, store::StoreError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "storage failure"))?
        .map_err(store_err)
}

fn store_err(e: store::StoreError) -> axum::response::Response {
    match e {
        store::StoreError::AccountQuota | store::StoreError::OpsQuota => {
            err(StatusCode::TOO_MANY_REQUESTS, "quota exceeded")
        }
        other => {
            tracing::warn!("relay store failure: {other}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "storage failure")
        }
    }
}

// ---------------------------------------------------------------------------
// Auth routes
// ---------------------------------------------------------------------------

async fn challenge(
    State(state): State<SharedState>,
    Json(req): Json<wl_protocol::ChallengeRequest>,
) -> axum::response::Response {
    match state.auth.issue_challenge(&req.public_key) {
        Ok((nonce, expires_at)) => {
            // Register the account on first challenge (public key =
            // account id; zero personal data). The store enforces a
            // registration cap (unauthenticated DoS guard).
            let key = req.public_key.clone();
            let owned = state.clone();
            if let Err(resp) = blocking(move || owned.blobs.register_account(&key)).await {
                return resp;
            }
            (
                StatusCode::OK,
                Json(wl_protocol::Challenge { nonce, expires_at }),
            )
                .into_response()
        }
        Err(AuthError::BadPublicKey) => err(StatusCode::BAD_REQUEST, "invalid public key"),
        Err(AuthError::RateLimited) => {
            err(StatusCode::TOO_MANY_REQUESTS, "too many challenge requests")
        }
        Err(e) => {
            tracing::warn!("relay challenge failure: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "auth failure")
        }
    }
}

async fn verify(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<wl_protocol::VerifyRequest>,
) -> axum::response::Response {
    // The client re-sends the challenge parts it signed via headers
    // (X-Nonce / X-Expires) alongside the signature in the body.
    let (Some(nonce), Some(expires)) = (
        headers.get("x-nonce").and_then(|v| v.to_str().ok()),
        headers
            .get("x-expires")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<i64>().ok()),
    ) else {
        return err(
            StatusCode::BAD_REQUEST,
            "missing x-nonce / x-expires headers",
        );
    };
    let Ok(sig) = hex::decode(&req.signature) else {
        return err(StatusCode::BAD_REQUEST, "bad signature encoding");
    };
    let Ok(sig) = <[u8; 64]>::try_from(sig) else {
        return err(StatusCode::BAD_REQUEST, "bad signature length");
    };
    match state.auth.verify(&req.public_key, nonce, expires, &sig) {
        Ok((token, expires_at)) => (
            StatusCode::OK,
            Json(wl_protocol::SessionToken { token, expires_at }),
        )
            .into_response(),
        Err(AuthError::BadChallenge) => {
            err(StatusCode::UNAUTHORIZED, "challenge expired or unknown")
        }
        Err(AuthError::BadSignature) => err(StatusCode::UNAUTHORIZED, "signature rejected"),
        Err(_) => err(StatusCode::INTERNAL_SERVER_ERROR, "auth failure"),
    }
}

// ---------------------------------------------------------------------------
// Sync routes (bearer-token gated; blind blobs)
// ---------------------------------------------------------------------------

#[allow(clippy::result_large_err)]
fn bearer_account(
    state: &SharedState,
    headers: &HeaderMap,
) -> Result<String, axum::response::Response> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "missing bearer token"))?;
    state
        .auth
        .validate(token)
        .map_err(|_| err(StatusCode::UNAUTHORIZED, "invalid or expired token"))
}

/// Validates one pushed op's routing envelope. The relay can never
/// read the ciphertext, but malformed headers have broken clients
/// before (unpadded HLC text inverts TEXT ordering; unknown tables
/// wedge merges), so the boundary rejects them with 400 instead of
/// storing poison every peer would choke on.
fn validate_push_op(op: &wl_protocol::PushOp) -> Result<(), &'static str> {
    if op.operation_id.is_empty() || op.operation_id.len() > MAX_HEADER_LEN {
        return Err("bad operation id");
    }
    if op.record_id.is_empty() || op.record_id.len() > MAX_HEADER_LEN {
        return Err("bad record id");
    }
    if wl_core::crdt::CrdtTable::from_str(&op.table).is_none() {
        return Err("unknown table");
    }
    // Canonical fixed-width HLC (`pt(20).ctr(5).dev(5)`): parse and
    // require the re-render to round-trip, so unpadded or malformed
    // timestamps can never enter TEXT-ordered storage.
    match wl_core::hlc::HlcTimestamp::parse(&op.hlc) {
        Ok(ts) if ts.to_string() == op.hlc => {}
        _ => return Err("non-canonical hlc"),
    }
    Ok(())
}

async fn push(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<wl_protocol::PushRequest>,
) -> axum::response::Response {
    let account = match bearer_account(&state, &headers) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    if req.ops.len() > MAX_OPS_PER_PUSH {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "too many ops in one push");
    }
    let mut ops = Vec::with_capacity(req.ops.len());
    for op in &req.ops {
        let Ok(sealed) = base64::engine::general_purpose::STANDARD.decode(&op.sealed_b64) else {
            return err(StatusCode::BAD_REQUEST, "invalid base64 payload");
        };
        if sealed.len() > MAX_SEALED_BYTES {
            return err(StatusCode::PAYLOAD_TOO_LARGE, "op payload too large");
        }
        if let Err(msg) = validate_push_op(op) {
            return err(StatusCode::BAD_REQUEST, msg);
        }
        ops.push(StoredOp {
            operation_id: op.operation_id.clone(),
            account: account.clone(),
            hlc: op.hlc.clone(),
            table: op.table.clone(),
            record_id: op.record_id.clone(),
            sealed,
        });
    }
    let owned = state.clone();
    match blocking(move || owned.blobs.insert_ops(&ops)).await {
        Ok(outcome) => (
            StatusCode::OK,
            Json(wl_protocol::PushResponse {
                accepted: outcome.accepted,
                duplicates: outcome.duplicates,
            }),
        )
            .into_response(),
        Err(resp) => resp,
    }
}

async fn pull(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<wl_protocol::PullRequest>,
) -> axum::response::Response {
    let account = match bearer_account(&state, &headers) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let limit = req.limit.clamp(1, PULL_HARD_CAP);
    let since_hlc = req.since_hlc.clone();
    let since_op_id = req.since_op_id.clone();
    let owned = state.clone();
    match blocking(move || {
        owned
            .blobs
            .pull_ops(&account, &since_hlc, &since_op_id, limit)
    })
    .await
    {
        Ok(stored) => {
            let exhausted = (stored.len() as u32) < limit;
            let ops: Vec<wl_protocol::PushOp> = stored
                .iter()
                .map(|o| wl_protocol::PushOp {
                    operation_id: o.operation_id.clone(),
                    hlc: o.hlc.clone(),
                    table: o.table.clone(),
                    record_id: o.record_id.clone(),
                    sealed_b64: base64::engine::general_purpose::STANDARD.encode(&o.sealed),
                })
                .collect();
            // Composite cursor: (hlc, operation_id) of the last op.
            // Same-HLC ties are advanced by the op id, so no op is
            // unreachable no matter how many share a timestamp.
            let (next_cursor, next_op_id) = match stored.last() {
                Some(o) => (o.hlc.clone(), o.operation_id.clone()),
                None => (req.since_hlc.clone(), req.since_op_id.clone()),
            };
            (
                StatusCode::OK,
                Json(wl_protocol::PullResponse {
                    ops,
                    next_cursor,
                    next_op_id,
                    exhausted,
                }),
            )
                .into_response()
        }
        Err(resp) => resp,
    }
}

// ---------------------------------------------------------------------------
// Store opening helper (used by the binary)
// ---------------------------------------------------------------------------

#[cfg(feature = "sqlite")]
pub fn open_store(db_path: &str) -> Result<Box<dyn BlobStore>, store::StoreError> {
    if db_path == ":memory:" {
        Ok(Box::new(SqliteStore::open_in_memory()?))
    } else {
        Ok(Box::new(SqliteStore::open(std::path::Path::new(db_path))?))
    }
}

/// Test-support re-exports (integration tests in wl-sync).
pub use auth::AuthState as AuthForTest;
#[cfg(feature = "sqlite")]
pub use store::sqlite_backend::SqliteStore as SqliteForTest;
pub type AppStateForTest = AppState;

pub fn router_for_test(state: SharedState) -> Router {
    router(state)
}
