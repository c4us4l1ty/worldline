//! Wire protocol shared by client and relay (PRD §3.2).
//!
//! Auth: challenge → Ed25519 signature → opaque bearer session token
//! (PASETO wrapping is future work — see PRD-DELTAS §4, not v0.1).
//! Data: opaque encrypted CRDT blobs. The relay validates signatures
//! and routes ciphertext; it can never read payloads.

use serde::{Deserialize, Serialize};

pub const MAX_BATCH_OPS: usize = 500;
pub const MAX_SEALED_BYTES: usize = 256 * 1024;
pub const MAX_SEALED_B64: usize = MAX_SEALED_BYTES.div_ceil(3) * 4;
pub const MAX_HEADER_LEN: usize = 128;
pub const MAX_REQUEST_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PULL_BYTES: usize = 1024 * 1024;

pub fn valid_hlc(hlc: &str) -> bool {
    let bytes = hlc.as_bytes();
    if bytes.len() != 32
        || bytes[20] != b'.'
        || bytes[26] != b'.'
        || !bytes
            .iter()
            .enumerate()
            .all(|(i, b)| i == 20 || i == 26 || b.is_ascii_digit())
    {
        return false;
    }
    hlc[..20].parse::<u64>().is_ok()
        && hlc[21..26].parse::<u16>().is_ok()
        && hlc[27..].parse::<u16>().is_ok()
}

pub fn valid_cursor(hlc: &str, op_id: &str) -> bool {
    op_id.len() <= MAX_HEADER_LEN
        && if hlc.is_empty() {
            op_id.is_empty()
        } else {
            valid_hlc(hlc)
        }
}

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
pub fn challenge_signing_payload(nonce_hex: &str, expires_at: i64) -> Vec<u8> {
    checked_challenge_signing_payload(nonce_hex, expires_at)
        .map(|payload| payload.to_vec())
        .unwrap_or_default()
}

pub fn checked_challenge_signing_payload(nonce_hex: &str, expires_at: i64) -> Option<[u8; 40]> {
    let mut payload = [0u8; 40];
    hex::decode_to_slice(nonce_hex, &mut payload[..32]).ok()?;
    payload[32..].copy_from_slice(&expires_at.to_be_bytes());
    Some(payload)
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
    fn malformed_nonce_yields_empty_payload() {
        assert!(challenge_signing_payload("zz", 42).is_empty());
        assert!(challenge_signing_payload("", 42).is_empty());
        assert!(challenge_signing_payload(&"d".repeat(63), 42).is_empty());
    }

    #[test]
    fn checked_payload_accepts_only_exact_hex_nonce() {
        let nonce = "ab".repeat(32);
        let p = checked_challenge_signing_payload(&nonce, -1).unwrap();
        assert_eq!(p.len(), 40);
        assert_eq!(&p[32..], &(-1i64).to_be_bytes());
        assert!(checked_challenge_signing_payload(&"ab".repeat(31), 1).is_none());
        assert!(checked_challenge_signing_payload("nothex!", 1).is_none());
        assert!(checked_challenge_signing_payload("", 1).is_none());
    }

    #[test]
    fn valid_hlc_matches_canonical_fixed_width_encoding() {
        assert!(valid_hlc("00000000000000000001.00002.00003"));
        assert!(!valid_hlc("1.2.3"));
        assert!(!valid_hlc("not-an-hlc"));
        assert!(!valid_hlc(""));
        assert!(!valid_hlc("18446744073709551616.00000.00000"));
        assert!(!valid_hlc("00000000000000000001.65536.00000"));
        assert!(valid_hlc("18446744073709551615.65535.65535"));
    }

    #[test]
    fn valid_cursor_bounds_and_pairs() {
        assert!(valid_cursor("", ""));
        assert!(!valid_cursor("", "op"));
        assert!(valid_cursor("00000000000000000001.00000.00000", ""));
        assert!(!valid_cursor(
            "00000000000000000001.00000.00000",
            &"x".repeat(129)
        ));
        assert!(valid_cursor("00000000000000000001.00000.00000", "op-1"));
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
