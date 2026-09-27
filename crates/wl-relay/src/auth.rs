//! Relay authentication: unguessable challenge → Ed25519 signature →
//! short-lived opaque bearer token. No stateless PASETO/JWT wrapping.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rand::RngCore;
use wl_core::poison::LockRecover;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("unknown account")]
    UnknownAccount,
    #[error("challenge not found or expired")]
    BadChallenge,
    #[error("signature verification failed")]
    BadSignature,
    #[error("invalid public key")]
    BadPublicKey,
    #[error("token invalid")]
    BadToken,
    /// Challenge flood guard: too many unauthenticated challenge
    /// requests from one public key (or globally) in the window.
    #[error("too many challenge requests — slow down")]
    RateLimited,
}

/// Challenge lifetime.
pub const CHALLENGE_TTL: Duration = Duration::from_secs(120);
/// Session token lifetime.
pub const SESSION_TTL: Duration = Duration::from_secs(3600);

/// Max in-flight challenges per account. A real client needs ONE at
/// a time; anything beyond this is a flood.
pub const MAX_CHALLENGES_PER_ACCOUNT: usize = 4;
/// Global in-flight challenge ceiling (memory-DoS bound). Reaching it
/// means the relay is under flood; new challenges are refused until
/// TTLs expire.
///
/// Sized for the HOT PATH, not for headroom. Every `/auth/challenge`
/// walks this table twice (an amortised expiry sweep and a per-account
/// count) on an unauthenticated request while holding the mutex, and
/// `ACCOUNT_CAP` is 10 000 — so 4 challenges per account can never
/// exceed 40 000 live rows in practice. A ceiling far above that buys
/// nothing and turns table growth directly into per-request CPU: at
/// 100 000 each challenge cost ~1 ms of convoyed work that a caller who
/// had authenticated nothing could demand forever.
pub const MAX_CHALLENGES_GLOBAL: usize = 4_000;

pub const MAX_SESSIONS_PER_ACCOUNT: usize = 8;
pub const MAX_SESSIONS_GLOBAL: usize = 100_000;

pub const MAX_TOKEN_LEN: usize = 512;

/// GC every N calls of the hot path that can grow these maps. Keeps the
/// token path cheap while preventing unbounded accumulation under
/// pull-only traffic (expired rows would otherwise linger until the next
/// challenge anywhere on the relay).
const VALIDATE_GC_INTERVAL: u64 = 128;
/// Same, for the challenge table — which is the one an UNAUTHENTICATED
/// caller can grow, so its sweep is the one that most needs amortising.
const CHALLENGE_GC_INTERVAL: u64 = 64;

/// In-flight challenges: nonce → (public_key, expires_at).
/// Sessions: token → (public_key, expires_at).
pub struct AuthState {
    challenges: Mutex<HashMap<String, (String, i64)>>,
    sessions: Mutex<HashMap<String, (String, i64)>>,
    challenge_calls: AtomicU64,
    validate_calls: AtomicU64,
}

impl AuthState {
    pub fn new() -> Self {
        Self {
            challenges: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            challenge_calls: AtomicU64::new(0),
            validate_calls: AtomicU64::new(0),
        }
    }

    /// Issues an unguessable cryptographic challenge for an account.
    /// Registering on first sight is intentional (zero-knowledge:
    /// the public key IS the account). Refuses floods: bounded in-flight
    /// challenges per account AND globally.
    pub fn issue_challenge(&self, public_key: &str) -> Result<(String, i64), AuthError> {
        if public_key.len() != 64
            || !public_key
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(AuthError::BadPublicKey);
        }
        let mut nonce_bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = hex::encode(nonce_bytes);
        let expires_at = now_secs() + CHALLENGE_TTL.as_secs() as i64;
        {
            let mut challenges = self.challenges.lock_recover();
            // Full expiry sweep is THROTTLED, not per-request.
            //
            // `/auth/challenge` is unauthenticated, and the sweep walks
            // every live challenge under the mutex. At the old global cap
            // that was 100 000 entries — so filling the table once turned
            // every later challenge request into ~1 ms of convoyed work
            // from a caller who has authenticated nothing. Sweeping every
            // CHALLENGE_GC_INTERVAL-th request amortises the cost over the
            // window that filled the table; between sweeps an expired
            // entry keeps occupying its slot, which makes the caps read
            // slightly conservative. That is the right direction to be
            // wrong in.
            if self
                .challenge_calls
                .fetch_add(1, Ordering::Relaxed)
                .is_multiple_of(CHALLENGE_GC_INTERVAL)
            {
                let now = now_secs();
                challenges.retain(|_, (_, e)| *e > now);
            }
            let per_account = challenges
                .values()
                .filter(|(pk, _)| *pk == public_key)
                .count();
            if per_account >= MAX_CHALLENGES_PER_ACCOUNT {
                return Err(AuthError::RateLimited);
            }
            if challenges.len() >= MAX_CHALLENGES_GLOBAL {
                return Err(AuthError::RateLimited);
            }
            challenges.insert(nonce.clone(), (public_key.to_string(), expires_at));
        }
        // No `gc_sessions()` here. It swept the entire session map on an
        // unauthenticated request for no benefit: `validate` already runs
        // the same sweep on a throttle, and it is the only caller whose
        // growth an unauthenticated peer can actually drive.
        Ok((nonce, expires_at))
    }

    /// Verifies a signed challenge; on success mints a session token.
    pub fn verify(
        &self,
        public_key: &str,
        nonce: &str,
        expires_at: i64,
        signature: &[u8],
    ) -> Result<(String, i64), AuthError> {
        {
            let challenges = self.challenges.lock_recover();
            let (expected_pk, stored_exp) = challenges.get(nonce).ok_or(AuthError::BadChallenge)?;
            if expected_pk != public_key || *stored_exp != expires_at {
                return Err(AuthError::BadChallenge);
            }
        }
        if expires_at <= now_secs() {
            // Consume the challenge either way (one-shot).
            self.challenges.lock_recover().remove(nonce);
            return Err(AuthError::BadChallenge);
        }

        // Consume the challenge BEFORE the expensive part, not after.
        //
        // The claim used to run only once `verify_strict` had already
        // succeeded, so a FAILED signature left the nonce in the map and
        // the four nonces `MAX_CHALLENGES_PER_ACCOUNT` allows served an
        // unbounded number of retries. Each retry pays point
        // decompression plus a double-scalar multiply — `verify_strict`
        // is the deliberately slow variant, ~50-100x a null request — on
        // an UNAUTHENTICATED route, and nonces renew every
        // CHALLENGE_TTL. One keypair could therefore hold the relay's
        // CPU indefinitely. A nonce is a 256-bit secret the client
        // holds, so burning it on a bad signature costs a real client
        // nothing: it asks for another.
        let claimed = self.challenges.lock_recover().remove(nonce);
        let claimed = claimed.ok_or(AuthError::BadChallenge)?;
        if claimed.0 != public_key || claimed.1 != expires_at || expires_at <= now_secs() {
            return Err(AuthError::BadChallenge);
        }

        // Ed25519 verify over nonce ‖ expiry (matches client signing).
        let payload = wl_protocol::checked_challenge_signing_payload(nonce, expires_at)
            .ok_or(AuthError::BadChallenge)?;
        let pk_bytes = hex::decode(public_key).map_err(|_| AuthError::BadPublicKey)?;
        let arr: [u8; 32] = pk_bytes.try_into().map_err(|_| AuthError::BadPublicKey)?;
        let vk =
            ed25519_dalek::VerifyingKey::from_bytes(&arr).map_err(|_| AuthError::BadPublicKey)?;
        let sig: [u8; 64] = signature.try_into().map_err(|_| AuthError::BadSignature)?;
        vk.verify_strict(&payload, &ed25519_dalek::Signature::from_bytes(&sig))
            .map_err(|_| AuthError::BadSignature)?;

        // Tokens are opaque, random bearer credentials retained server-side.
        // Purely random: the old `"{counter}-{uuid}"` shape prefixed a
        // per-process, `fetch_add`-from-zero sequence, which handed every
        // token holder a session counter and an instance fingerprint, and
        // grew monotonically for the life of the process.
        let sess_expires = now_secs() + SESSION_TTL.as_secs() as i64;
        let mut token_bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut token_bytes);
        let token_id = hex::encode(token_bytes);
        let mut sessions = self.sessions.lock_recover();
        let now = now_secs();
        sessions.retain(|_, (_, e)| *e > now);
        let per_account = sessions.values().filter(|(pk, _)| pk == public_key).count();
        if per_account >= MAX_SESSIONS_PER_ACCOUNT || sessions.len() >= MAX_SESSIONS_GLOBAL {
            return Err(AuthError::RateLimited);
        }
        sessions.insert(token_id.clone(), (public_key.to_string(), sess_expires));
        Ok((token_id, sess_expires))
    }

    /// Validates a bearer token → owning account (opaque pubkey hex).
    /// Expired rows are removed on sight, and a cheap counter triggers
    /// a full GC sweep periodically — pull-only clients no longer
    /// accumulate stale sessions until some other client challenges.
    pub fn validate(&self, token: &str) -> Result<String, AuthError> {
        if token.is_empty() || token.len() > MAX_TOKEN_LEN {
            return Err(AuthError::BadToken);
        }
        let now = now_secs();
        let pk = {
            let mut sessions = self.sessions.lock_recover();
            match sessions.get(token).cloned() {
                Some((pk, exp)) => {
                    if exp <= now {
                        // Remove the dead row on sight: pull-only clients
                        // must not accumulate expired sessions.
                        sessions.remove(token);
                        None
                    } else {
                        Some(pk)
                    }
                }
                None => None,
            }
        };
        let Some(pk) = pk else {
            return Err(AuthError::BadToken);
        };
        // Throttled sweep: amortized O(1) per validate call.
        if self.validate_calls.fetch_add(1, Ordering::Relaxed) % VALIDATE_GC_INTERVAL
            == VALIDATE_GC_INTERVAL - 1
        {
            self.gc_sessions();
        }
        Ok(pk)
    }

    /// Drops expired challenges/sessions.
    pub fn gc(&self) {
        let now = now_secs();
        self.challenges.lock_recover().retain(|_, (_, e)| *e > now);
        self.gc_sessions();
    }

    fn gc_sessions(&self) {
        let now = now_secs();
        self.sessions.lock_recover().retain(|_, (_, e)| *e > now);
    }

    /// Test support: force a session row to expired (TTLs are real
    /// wall-clock 1h; tests need the row dead NOW).
    #[doc(hidden)]
    pub fn expire_session_for_test(&self, token: &str) {
        if let Some(entry) = self.sessions.lock_recover().get_mut(token) {
            entry.1 = 0;
        }
    }

    /// Test support: does a session row still exist (expired or not)?
    #[doc(hidden)]
    pub fn session_row_exists(&self, token: &str) -> bool {
        self.sessions.lock_recover().contains_key(token)
    }
}

impl Default for AuthState {
    fn default() -> Self {
        Self::new()
    }
}

fn now_secs() -> i64 {
    // A broken host clock must degrade (fail-closed expiry checks),
    // never panic the relay off the edge.
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn fresh_keypair() -> (String, SigningKey) {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        (hex::encode(sk.verifying_key().as_bytes()), sk)
    }

    #[test]
    fn challenge_verify_flow() {
        let auth = AuthState::new();
        let (pk, sk) = fresh_keypair();
        let (nonce, expires) = auth.issue_challenge(&pk).unwrap();
        let payload = wl_protocol::challenge_signing_payload(&nonce, expires);
        let sig = sk.sign(&payload).to_bytes();
        let (token, exp) = auth.verify(&pk, &nonce, expires, &sig).unwrap();
        assert!(!token.is_empty());
        assert!(exp > now_secs());
        assert_eq!(auth.validate(&token).unwrap(), pk);
    }

    #[test]
    fn challenge_is_one_shot() {
        let auth = AuthState::new();
        let (pk, sk) = fresh_keypair();
        let (nonce, expires) = auth.issue_challenge(&pk).unwrap();
        let payload = wl_protocol::challenge_signing_payload(&nonce, expires);
        let sig = sk.sign(&payload).to_bytes();
        auth.verify(&pk, &nonce, expires, &sig).unwrap();
        // Replay fails.
        assert!(matches!(
            auth.verify(&pk, &nonce, expires, &sig),
            Err(AuthError::BadChallenge)
        ));
    }

    #[test]
    fn wrong_signature_rejected() {
        let auth = AuthState::new();
        let (pk, _sk) = fresh_keypair();
        let (nonce, expires) = auth.issue_challenge(&pk).unwrap();
        let evil = SigningKey::from_bytes(&[9u8; 32]);
        let payload = wl_protocol::challenge_signing_payload(&nonce, expires);
        let sig = evil.sign(&payload).to_bytes();
        assert!(matches!(
            auth.verify(&pk, &nonce, expires, &sig),
            Err(AuthError::BadSignature)
        ));
    }

    #[test]
    fn bad_public_key_rejected() {
        let auth = AuthState::new();
        assert!(auth.issue_challenge("zzz").is_err());
        assert!(auth.issue_challenge("").is_err());
        // 32 bytes of valid hex is accepted.
        assert!(auth.issue_challenge(&"ab".repeat(32)).is_ok());
    }

    #[test]
    fn token_for_unknown_is_invalid() {
        let auth = AuthState::new();
        assert!(matches!(auth.validate("nope"), Err(AuthError::BadToken)));
    }

    #[test]
    fn challenge_flood_is_rate_limited_per_account() {
        let auth = AuthState::new();
        let (pk, _) = fresh_keypair();
        for _ in 0..MAX_CHALLENGES_PER_ACCOUNT {
            assert!(auth.issue_challenge(&pk).is_ok());
        }
        // The next unauthenticated challenge for the SAME key is refused.
        assert!(matches!(
            auth.issue_challenge(&pk),
            Err(AuthError::RateLimited)
        ));
        // A different (legitimate) key is unaffected.
        let sk = SigningKey::from_bytes(&[11u8; 32]);
        let other = hex::encode(sk.verifying_key().as_bytes());
        assert!(auth.issue_challenge(&other).is_ok());
    }

    #[test]
    fn session_flood_is_rate_limited_per_account() {
        let auth = AuthState::new();
        let (pk, sk) = fresh_keypair();
        for _ in 0..MAX_SESSIONS_PER_ACCOUNT {
            let (nonce, expires) = auth.issue_challenge(&pk).unwrap();
            let payload = wl_protocol::challenge_signing_payload(&nonce, expires);
            let sig = sk.sign(&payload).to_bytes();
            auth.verify(&pk, &nonce, expires, &sig).unwrap();
        }
        let (nonce, expires) = auth.issue_challenge(&pk).unwrap();
        let payload = wl_protocol::challenge_signing_payload(&nonce, expires);
        let sig = sk.sign(&payload).to_bytes();
        assert!(matches!(
            auth.verify(&pk, &nonce, expires, &sig),
            Err(AuthError::RateLimited)
        ));
        let sk2 = SigningKey::from_bytes(&[13u8; 32]);
        let other = hex::encode(sk2.verifying_key().as_bytes());
        let (nonce2, expires2) = auth.issue_challenge(&other).unwrap();
        let sig2 = sk2
            .sign(&wl_protocol::challenge_signing_payload(&nonce2, expires2))
            .to_bytes();
        assert!(auth.verify(&other, &nonce2, expires2, &sig2).is_ok());
    }

    #[test]
    fn oversized_token_is_rejected_without_map_lookup() {
        let auth = AuthState::new();
        let long = "a".repeat(513);
        assert!(matches!(auth.validate(&long), Err(AuthError::BadToken)));
    }
    #[test]
    fn public_keys_have_one_canonical_account_representation() {
        let auth = AuthState::new();
        let (pk, _) = fresh_keypair();
        assert!(matches!(
            auth.issue_challenge(&pk.to_uppercase()),
            Err(AuthError::BadPublicKey)
        ));
        assert!(auth.issue_challenge(&pk).is_ok());
    }

    #[test]
    fn expired_challenge_cannot_create_session() {
        let auth = AuthState::new();
        let (pk, sk) = fresh_keypair();
        let (nonce, _) = auth.issue_challenge(&pk).unwrap();
        let expiry = now_secs();
        auth.challenges.lock_recover().get_mut(&nonce).unwrap().1 = expiry;
        let sig = sk
            .sign(&wl_protocol::challenge_signing_payload(&nonce, expiry))
            .to_bytes();
        assert!(matches!(
            auth.verify(&pk, &nonce, expiry, &sig),
            Err(AuthError::BadChallenge)
        ));
    }

    #[test]
    fn validate_replaces_expired_sessions_on_sight() {
        // An expired session row must be REMOVED by validate (the
        // row, not just the verdict) so pull-only traffic cannot
        // accumulate dead rows.
        let auth = AuthState::new();
        let (pk, sk) = fresh_keypair();
        let (nonce, expires) = auth.issue_challenge(&pk).unwrap();
        let payload = wl_protocol::challenge_signing_payload(&nonce, expires);
        let sig = sk.sign(&payload).to_bytes();
        let (token, _) = auth.verify(&pk, &nonce, expires, &sig).unwrap();
        // Force-expire the row directly (TTL is 1h; tests can't wait).
        auth.expire_session_for_test(&token);
        assert!(matches!(auth.validate(&token), Err(AuthError::BadToken)));
        // The row is gone now — the map no longer holds it.
        assert!(!auth.session_row_exists(&token));
    }
}
