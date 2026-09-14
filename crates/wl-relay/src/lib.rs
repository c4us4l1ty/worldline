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
            // account id; zero personal data).
            if let Err(e) = state.blobs.register_account(&req.public_key) {
                return err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
            }
            (
                StatusCode::OK,
                Json(wl_protocol::Challenge { nonce, expires_at }),
            )
                .into_response()
        }
        Err(AuthError::BadPublicKey) => err(StatusCode::BAD_REQUEST, "invalid public key"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
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

async fn push(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(req): Json<wl_protocol::PushRequest>,
) -> axum::response::Response {
    let account = match bearer_account(&state, &headers) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let mut ops = Vec::with_capacity(req.ops.len());
    for op in &req.ops {
        let Ok(sealed) = base64::engine::general_purpose::STANDARD.decode(&op.sealed_b64) else {
            return err(StatusCode::BAD_REQUEST, "invalid base64 payload");
        };
        ops.push(StoredOp {
            operation_id: op.operation_id.clone(),
            account: account.clone(),
            hlc: op.hlc.clone(),
            table: op.table.clone(),
            record_id: op.record_id.clone(),
            sealed,
        });
    }
    match state.blobs.insert_ops(&ops) {
        Ok(accepted) => {
            (StatusCode::OK, Json(wl_protocol::PushResponse { accepted })).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
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
    match state.blobs.pull_ops(&account, &req.since_hlc, limit) {
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
            let next_cursor = stored
                .last()
                .map(|o| o.hlc.clone())
                .unwrap_or_else(|| req.since_hlc.clone());
            (
                StatusCode::OK,
                Json(wl_protocol::PullResponse {
                    ops,
                    next_cursor,
                    exhausted,
                }),
            )
                .into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
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
pub use store::sqlite_backend::SqliteStore as SqliteForTest;
pub type AppStateForTest = AppState;

pub fn router_for_test(state: SharedState) -> Router {
    router(state)
}
