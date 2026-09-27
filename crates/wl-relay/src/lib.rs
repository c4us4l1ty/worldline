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

use axum::extract::{DefaultBodyLimit, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use base64::Engine;
use serde_json::json;
use std::time::Duration;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::timeout::TimeoutLayer;

use auth::{AuthError, AuthState};
use store::{BlobStore, StoredOp};

#[cfg(feature = "sqlite")]
use store::sqlite_backend::SqliteStore;

const PULL_HARD_CAP: u32 = wl_protocol::MAX_BATCH_OPS as u32;
/// Hard cap on ops per push request (storage-exhaustion + CPU bound).
const MAX_OPS_PER_PUSH: usize = wl_protocol::MAX_BATCH_OPS;
const MAX_SEALED_BYTES: usize = wl_protocol::MAX_SEALED_BYTES;
const MAX_SEALED_B64: usize = wl_protocol::MAX_SEALED_B64;
/// Routing-header length bound (ids are UUID-shaped; generous ceiling).
const MAX_HEADER_LEN: usize = wl_protocol::MAX_HEADER_LEN;

/// Wall-clock ceiling on a single request, body included.
///
/// Without it the relay has no bound on how long a connection can stay
/// open: a client that dribbles a 2 MiB body one byte at a time holds a
/// tokio task and its read buffer for as long as it likes, and every
/// such connection costs memory whether or not it ever does any work.
/// That is unbounded RAM growth on an UNAUTHENTICATED socket, against a
/// product whose whole point is a near-zero idle footprint.
///
/// Generous on purpose: the slowest legitimate route is a 500-op push
/// that may touch several hundred thousand rows, and a legitimate client
/// behind a slow uplink must not be cut off mid-write. This bounds the
/// abusive case, not the slow one.
const REQUEST_DEADLINE: Duration = Duration::from_secs(30);

/// Requests served at once. Bounds task count, per-request buffers, and
/// the blast radius of a burst; excess requests queue at the layer
/// instead of each allocating a task.
///
/// Sized to the storage backends, not in the abstract. It used to be
/// 256 against a 16-connection Postgres pool, so a brief database stall
/// parked 240 of the 256 permits *waiting* for a connection and the
/// 30 s deadline fired — turning backpressure into 504s. 64 keeps a
/// healthy margin over the pool (so real concurrency is served) while
/// still leaving a ceiling a stalled database cannot exhaust.
const MAX_INFLIGHT_REQUESTS: usize = 64;

/// Concurrent TCP connections. Separate from [`MAX_INFLIGHT_REQUESTS`]
/// because a connection that has not finished its request head is not
/// yet a request: it holds a tokio task and hyper's read buffer without
/// ever acquiring a service permit, so the per-request limit does not
/// see it at all. A client that opens a socket and dribbles one byte of
/// header every 29 s is parked in the header phase forever. Dropping
/// the excess at accept time is what actually bounds the memory.
pub const MAX_CONNECTIONS: usize = 128;

/// Deadline for reading a request head (method, URI, headers) after a
/// connection is accepted. Only applies until the head is complete, so
/// a long-lived keep-alive connection is not cut off — but a partial
/// head is dropped rather than parked.
pub const HEADER_READ_DEADLINE: Duration = Duration::from_secs(15);

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
        .layer(DefaultBodyLimit::max(wl_protocol::MAX_REQUEST_BYTES))
        .layer(ConcurrencyLimitLayer::new(MAX_INFLIGHT_REQUESTS))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::GATEWAY_TIMEOUT,
            REQUEST_DEADLINE,
        ))
        .with_state(state)
}

fn err(status: StatusCode, msg: &str) -> axum::response::Response {
    (status, Json(json!({"error": msg}))).into_response()
}

/// The account a request is acting for, resolved from the bearer token.
///
/// A `FromRequestParts` extractor, not a call at the top of the handler,
/// and that is the whole point. axum runs `FromRequestParts` extractors
/// before `FromRequest` ones, and `Json` is the latter — so a check
/// written in the handler body ran *after* the entire body had been
/// buffered and deserialised. A 2 MiB push body is roughly 31 800
/// `PushOp` structs (≈4 MB of live heap, since every field is a
/// `String`) and the per-request op-count check that would reject it
/// lives in the handler too, so 256 such requests saturate the whole
/// concurrency limit on JSON parsing that no credential was ever
/// allowed to start. Anonymous clients, the full body limit, and the
/// CPU of a full parse — for nothing.
pub struct Account(pub String);

impl FromRequestParts<SharedState> for Account {
    type Rejection = axum::response::Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &SharedState,
    ) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or_else(|| err(StatusCode::UNAUTHORIZED, "missing bearer token"))?;
        let account = state
            .auth
            .validate(token)
            .map_err(|_| err(StatusCode::UNAUTHORIZED, "invalid or expired token"))?;
        Ok(Account(account))
    }
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
                // The nonce is already in the auth table and the client
                // never received it: anyone retrying against a degraded
                // store would leak one slot per attempt, and a slot for a
                // nonce nobody holds is exactly the waste that fills the
                // challenge table. Give it back before answering.
                state.auth.discard_challenge(&nonce);
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
    headers: axum::http::HeaderMap,
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
    if req.public_key.len() != 64 || nonce.len() != 64 || req.signature.len() > 256 {
        return err(StatusCode::BAD_REQUEST, "bad auth field length");
    }
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
        Err(AuthError::RateLimited) => err(StatusCode::TOO_MANY_REQUESTS, "too many sessions"),
        // A 64-char hex public key that is not a curve point is CLIENT
        // input, not a relay fault. `issue_challenge` accepts any such
        // string (the account id IS the public key, and the challenge
        // route is unauthenticated), so this was reachable by anyone and
        // answered 500 — polluting 5xx budgets and tripping client retry
        // logic, with no log line, because the only `tracing::warn!` in
        // this file is on the challenge path.
        Err(e @ AuthError::BadPublicKey) => {
            tracing::debug!("relay verify: bad public key: {e}");
            err(StatusCode::BAD_REQUEST, "invalid public key")
        }
        Err(e) => {
            tracing::warn!("relay verify failure: {e}");
            err(StatusCode::INTERNAL_SERVER_ERROR, "auth failure")
        }
    }
}

// ---------------------------------------------------------------------------
// Sync routes (bearer-token gated; blind blobs)
// ---------------------------------------------------------------------------

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
    Account(account): Account,
    Json(req): Json<wl_protocol::PushRequest>,
) -> axum::response::Response {
    if req.ops.len() > MAX_OPS_PER_PUSH {
        return err(StatusCode::PAYLOAD_TOO_LARGE, "too many ops in one push");
    }
    let mut ops = Vec::with_capacity(req.ops.len());
    for op in &req.ops {
        if op.sealed_b64.len() > MAX_SEALED_B64 {
            return err(StatusCode::PAYLOAD_TOO_LARGE, "op payload too large");
        }
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

/// Serialized size of one string, as `serde_json` emits it, without
/// materialising anything.
///
/// serde escapes `"` and `\` to two bytes, the five short escapes
/// (`\b \f \n \r \t`) to two, and every other C0 control to six
/// (`\u00XX`). Everything else — including every multi-byte UTF-8
/// sequence and `/` — is emitted verbatim, because `serde_json` does
/// not escape non-ASCII by default. Counting bytes, not chars, is
/// therefore exact, and so is the mapping from a byte to its escaped
/// width.
fn json_string_bytes(s: &str) -> usize {
    // 2 for the opening and closing quote.
    s.bytes()
        .map(|b| match b {
            b'"' | b'\\' | 0x08 | 0x0c | b'\n' | b'\r' | b'\t' => 2,
            0x00..=0x1f => 6,
            _ => 1,
        })
        .sum::<usize>()
        + 2
}

/// Serialized size of a `PushOp` exactly as `serde_json::to_string`
/// would produce it.
///
/// The pull budget used to MEASURE the op by serialising it —
/// `to_string(&op).map(|j| j.len())` — which allocates a complete copy of
/// every op (up to ~341 KiB for a max-sealed one) purely to count it,
/// and then `Json(PullResponse { .. })` serialised the same bytes a
/// second time. A full 8 MiB page therefore cost two complete
/// serialisations plus one transient allocation per op on the
/// authenticated hot path, for a number that is a pure function of the
/// five string lengths.
fn push_op_json_bytes(op: &wl_protocol::PushOp) -> usize {
    // Derived from the field names rather than counted by hand: the
    // scaffolding is two braces, four `,` field separators, and the
    // five `:` that follow the five quoted keys. Writing the key
    // lengths as string literals makes the term correct by
    // construction — a renamed or added field moves it — and the
    // exhaustive test below is what proves the mapping is the one
    // serde actually uses.
    const PUNCTUATION: usize = 2
        + 4
        + 5
        + "\"operation_id\"".len()
        + "\"hlc\"".len()
        + "\"table\"".len()
        + "\"record_id\"".len()
        + "\"sealed_b64\"".len();
    PUNCTUATION
        + json_string_bytes(&op.operation_id)
        + json_string_bytes(&op.hlc)
        + json_string_bytes(&op.table)
        + json_string_bytes(&op.record_id)
        + json_string_bytes(&op.sealed_b64)
}

async fn pull(
    State(state): State<SharedState>,
    Account(account): Account,
    Json(req): Json<wl_protocol::PullRequest>,
) -> axum::response::Response {
    let limit = req.limit.clamp(1, PULL_HARD_CAP);
    if !wl_protocol::valid_cursor(&req.since_hlc, &req.since_op_id) {
        return err(StatusCode::BAD_REQUEST, "bad pull cursor");
    }
    let since_hlc = req.since_hlc.clone();
    let since_op_id = req.since_op_id.clone();
    let owned = state.clone();
    match blocking(move || {
        // The byte budget goes ALL THE WAY DOWN. It used to be applied
        // here, after the store had already run
        // `SELECT … LIMIT 500` and collected every row into a `Vec` —
        // so a single authenticated account holding 500 max-sealed ops
        // made one pull materialise ~125 MiB, times 64 concurrent pulls.
        // The store now stops accumulating at the budget itself.
        owned.blobs.pull_ops(
            &account,
            &since_hlc,
            &since_op_id,
            limit,
            wl_protocol::MAX_PULL_BYTES,
        )
    })
    .await
    {
        Ok(stored) => {
            // SYNC-4: enforce the pull byte budget here, at the last
            // place that can. `limit` alone cannot bound the response:
            // 500 ops of 256 KiB each is ~167 MiB, so an op-count cap
            // alone is not a size limit. Ops are appended until the
            // budget is reached, which makes the emitted response
            // unconditionally within `MAX_PULL_BYTES`.
            //
            // Truncating here is pagination-safe: the cursor is taken
            // from the last op actually *included*, so the next pull
            // resumes exactly where this one stopped and nothing is
            // skipped. `exhausted` is false whenever we stopped early,
            // which is what tells the client to keep pulling.
            let mut ops: Vec<wl_protocol::PushOp> = Vec::new();
            let mut used = 0usize;
            let mut last_included: Option<&StoredOp> = None;
            for o in &stored {
                // Measure the op AS SERIALIZED, not as a sum of raw
                // field lengths. serde_json escapes a control character
                // as six bytes (``), so a header of 128 such
                // characters is 128 bytes of `str::len()` and 768 bytes
                // on the wire — and `operation_id`/`record_id` are
                // attacker-chosen up to `MAX_HEADER_LEN` each. The old
                // estimate therefore under-counted by up to 5x, the
                // response sailed past `MAX_PULL_BYTES`, and the client
                // — which reads with exactly that ceiling — rejected
                // the page. The cursor is only saved from a decoded
                // response, so it never advanced and the account could
                // never sync again. `MAX_OP_ENVELOPE_BYTES` is kept as
                // the punctuation/comma allowance on top of the exact
                // field costs.
                let op = wl_protocol::PushOp {
                    operation_id: o.operation_id.clone(),
                    hlc: o.hlc.clone(),
                    table: o.table.clone(),
                    record_id: o.record_id.clone(),
                    sealed_b64: base64::engine::general_purpose::STANDARD.encode(&o.sealed),
                };
                let cost = push_op_json_bytes(&op) + wl_protocol::MAX_OP_ENVELOPE_BYTES;
                if used + cost > wl_protocol::MAX_PULL_BYTES {
                    break;
                }
                used += cost;
                last_included = Some(o);
                ops.push(op);
            }
            // `exhausted` means "nothing remains after this batch", so
            // the client can stop pulling. It is true only when we included
            // everything the store returned AND the store returned fewer
            // ops than we asked for. Byte-truncation (`ops.len() <
            // stored.len()`) and a full page both mean "keep going".
            let exhausted = ops.len() == stored.len() && (stored.len() as u32) < limit;
            // Composite cursor: (hlc, operation_id) of the last op
            // *included*. Same-HLC ties are advanced by the op id, so no
            // op is unreachable no matter how many share a timestamp.
            let (next_cursor, next_op_id) = match last_included {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn op(operation_id: &str, record_id: &str, sealed: &[u8]) -> wl_protocol::PushOp {
        wl_protocol::PushOp {
            operation_id: operation_id.to_string(),
            hlc: "00000000000000000001.00000.00001".to_string(),
            table: "goals".to_string(),
            record_id: record_id.to_string(),
            sealed_b64: base64::engine::general_purpose::STANDARD.encode(sealed),
        }
    }

    /// The point of computing the serialized length instead of
    /// measuring it: the two must agree EXACTLY, on every input the
    /// attacker controls. A one-byte disagreement is a one-byte
    /// under-count of the pull budget, and the budget's whole job is to
    /// be an upper bound.
    #[test]
    fn the_measured_op_size_is_exactly_what_serde_emits() {
        // Hostile header content: the escape classes that make a raw
        // `str::len()` under-count by up to 5x, plus multi-byte UTF-8
        // and every C0 control serde escapes as `\u00XX`.
        let nasty = [
            "plain",
            "quote\"inside",
            "back\\slash",
            "new\nline\ttab\rcr\x08bs\x0cff",
            "\u{0}\u{1}\u{1f}controls",
            "emoji 🚀 and ünïcödé",
            "sl/ash/is/not/escaped",
            "",
        ];
        for operation_id in nasty {
            for record_id in nasty {
                for sealed in [b"".as_slice(), b"\x00\x01\xff", &[0x41; 300]] {
                    let o = op(operation_id, record_id, sealed);
                    let actual = serde_json::to_string(&o).unwrap().len();
                    assert_eq!(
                        push_op_json_bytes(&o),
                        actual,
                        "size mismatch for {operation_id:?}/{record_id:?}: measured {} vs serde {actual}",
                        push_op_json_bytes(&o)
                    );
                }
            }
        }
    }
}
