//! Wire protocol shared by client and relay (PRD §3.2).
//!
//! Auth: challenge → Ed25519 signature → opaque bearer session token
//! (PASETO wrapping is future work — see PRD-DELTAS §4, not v0.1).
//! Data: opaque encrypted CRDT blobs. The relay validates signatures
//! and routes ciphertext; it can never read payloads.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Auth handshake
// ---------------------------------------------------------------------------

/// POST /auth/challenge — request a nonce.
#[derive(Debug, Serialize, Deserialize)]
pub struct ChallengeRequest {
    /// Hex-encoded Ed25519 public key (the opaque Account ID).
    pub public_key: String,
}

/// Relay's cryptographic challenge: unguessable nonce + expiry.
#[derive(Debug, Serialize, Deserialize)]
pub struct Challenge {
    /// Hex-encoded 32-byte nonce.
    pub nonce: String,
    /// UNIX epoch seconds after which the challenge expires.
    pub expires_at: i64,
}

/// POST /auth/verify — signed challenge submission.
#[derive(Debug, Serialize, Deserialize)]
pub struct VerifyRequest {
    pub public_key: String,
    /// Signature over `nonce_bytes ‖ expires_at` where expiry is an
    /// i64 in big-endian bytes (see `challenge_signing_payload` — the
    /// single canonical constructor both sides must use).
    pub signature: String,
}

/// Successful auth: short-lived bearer session token.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionToken {
    /// PASETO v4.local token (or JWT in alternative deployments).
    pub token: String,
    /// Epoch seconds until expiry.
    pub expires_at: i64,
}

// ---------------------------------------------------------------------------
// CRDT transport (blind relay)
// ---------------------------------------------------------------------------

/// AAD routing header prefix bound into every sealed payload:
/// `table_name:record_id`. Relay only ever sees these fields plus
/// ciphertext — authenticated via AEAD, unreadable.
#[derive(Debug, Serialize, Deserialize)]
pub struct PushOp {
    /// Globally unique operation id.
    pub operation_id: String,
    /// HLC timestamp text (`pt.ctr.device`).
    pub hlc: String,
    /// Synced table name (routing only).
    pub table: String,
    /// Row id (routing only).
    pub record_id: String,
    /// ChaCha20-Poly1305 sealed payload (nonce ‖ tag ‖ ciphertext).
    /// Base64 over the [`wl_core::crypto::aead::Sealed`] wire bytes.
    pub sealed_b64: String,
}

/// POST /sync/push — batch push of pending outbox ops.
#[derive(Debug, Serialize, Deserialize)]
pub struct PushRequest {
    pub ops: Vec<PushOp>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PushResponse {
    /// Operation ids newly accepted (deduped: re-push is idempotent).
    pub accepted: Vec<String>,
    /// Operation ids the relay already held (idempotent re-push).
    /// A push whose response was lost leaves ops pending client-side;
    /// the duplicates list lets the outbox drain them on the retry.
    #[serde(default)]
    pub duplicates: Vec<String>,
}

/// POST /sync/pull — pull ops strictly after an HLC cursor.
#[derive(Debug, Serialize, Deserialize)]
pub struct PullRequest {
    /// Exclusive lower-bound HLC timestamp text; "" = from the beginning.
    /// Same-HLC ops are disambiguated by the cursor's operation id.
    pub since_hlc: String,
    /// When `since_hlc` is non-empty: the operation id of the last op
    /// the caller already holds at exactly `since_hlc` — ops sharing
    /// the cursor's HLC but sorting after this id are still returned.
    /// Omitted/empty for a fresh pull.
    #[serde(default)]
    pub since_op_id: String,
    /// Max ops returned (server enforces a hard cap too).
    pub limit: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PullResponse {
    pub ops: Vec<PushOp>,
    /// Cursor to pass as `since_hlc` next time (the batch's last op).
    pub next_cursor: String,
    /// Operation id to pass as `since_op_id` next time (the batch's
    /// last op id) — disambiguates same-HLC ties.
    pub next_op_id: String,
    /// true when no further ops exist beyond this batch.
    pub exhausted: bool,
}

/// Canonical challenge-signing payload: nonce bytes ‖ expiry bytes.
/// Both client and relay derive signing/verification material from
/// this single function — no format drift possible.
///
/// The nonce MUST be 64 hex chars (32 bytes) as issued by the relay;
/// anything else is a programming error. `debug_assert` catches it in
/// tests/dev; release decodes leniently but the relay only ever looks
/// up issued challenges first, so a malformed nonce can never verify.
pub fn challenge_signing_payload(nonce_hex: &str, expires_at: i64) -> Vec<u8> {
    debug_assert_eq!(
        nonce_hex.len(),
        64,
        "challenge nonce must be 32 bytes hex"
    );
    let mut buf = Vec::with_capacity(32 + 8);
    buf.extend_from_slice(&hex::decode(nonce_hex).unwrap_or_default());
    buf.extend_from_slice(&expires_at.to_be_bytes());
    buf
}

/// Standard error envelope.
#[derive(Debug, Serialize, Deserialize)]
pub struct ApiError {
    pub error: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_payload_is_deterministic() {
        // 32-byte nonce hex, exactly as the relay issues.
        let nonce = "deadbeef".repeat(8);
        assert_eq!(nonce.len(), 64);
        let a = challenge_signing_payload(&nonce, 42);
        let b = challenge_signing_payload(&nonce, 42);
        assert_eq!(a, b);
        assert_eq!(a.len(), 32 + 8);
        // Expiry encoded big-endian: stable across platforms.
        assert_eq!(&a[32..], &42i64.to_be_bytes());
    }

    #[test]
    fn serde_roundtrips() {
        let push = PushRequest {
            ops: vec![PushOp {
                operation_id: "op-1".into(),
                hlc: "1.2.3".into(),
                table: "directives".into(),
                record_id: "dir-9".into(),
                sealed_b64: "c2VhbGVk".into(),
            }],
        };
        let json = serde_json::to_string(&push).unwrap();
        let back: PushRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.ops[0].operation_id, "op-1");
        assert_eq!(back.ops[0].hlc, "1.2.3");

        let pull = PullRequest {
            since_hlc: "9.9.9".into(),
            since_op_id: "op-9".into(),
            limit: 100,
        };
        let back: PullRequest =
            serde_json::from_str(&serde_json::to_string(&pull).unwrap()).unwrap();
        assert_eq!(back.limit, 100);
        assert_eq!(back.since_op_id, "op-9");
        // since_op_id defaults when absent (older clients).
        let back: PullRequest =
            serde_json::from_str(&serde_json::json!({"since_hlc": "", "limit": 5}).to_string())
                .unwrap();
        assert_eq!(back.since_op_id, "");
    }
}
