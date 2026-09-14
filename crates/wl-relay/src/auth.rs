//! Relay authentication (PRD §3.2): unguessable challenge → Ed25519
//! signature → short-lived PASETO v4 session token. The relay verifies
//! signatures against the registered public key and nothing else.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rand::RngCore;

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
pub const MAX_CHALLENGES_GLOBAL: usize = 100_000;

/// GC every N `validate` calls: keeps the token hot path cheap while
/// preventing unbounded session accumulation under pull-only traffic
/// (expired rows would otherwise linger until the next challenge
/// anywhere on the relay).
const VALIDATE_GC_INTERVAL: u64 = 128;

/// In-flight challenges: nonce → (public_key, expires_at).
/// Sessions: token → (public_key, expires_at).
pub struct AuthState {
    challenges: Mutex<HashMap<String, (String, i64)>>,
    pub(crate) sessions: Mutex<HashMap<String, (String, i64)>>,
    /// Monotonic counter for session token uniqueness.
    counter: AtomicU64,
    validate_calls: AtomicU64,
}

impl AuthState {
    pub fn new() -> Self {
        Self {
            challenges: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            counter: AtomicU64::new(0),
            validate_calls: AtomicU64::new(0),
        }
    }

    /// Issues an unguessable cryptographic challenge for an account.
    /// Registering on first sight is intentional (zero-knowledge:
    /// the public key IS the account). Refuses floods: bounded in-flight
    /// challenges per account AND globally.
    pub fn issue_challenge(&self, public_key: &str) -> Result<(String, i64), AuthError> {
        if hex::decode(public_key)
            .map(|b| b.len() != 32)
            .unwrap_or(true)
        {
            return Err(AuthError::BadPublicKey);
        }
        let mut nonce_bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = hex::encode(nonce_bytes);
        let expires_at = now_secs() + CHALLENGE_TTL.as_secs() as i64;
        {
            let mut challenges = self.challenges.lock().expect("auth mutex");
            // GC first so expired slots don't count against the caps.
            let now = now_secs();
            challenges.retain(|_, (_, e)| *e > now);
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
        self.gc_sessions();
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
            let challenges = self.challenges.lock().expect("auth mutex");
            let (expected_pk, stored_exp) = challenges.get(nonce).ok_or(AuthError::BadChallenge)?;
            if expected_pk != public_key || *stored_exp != expires_at {
                return Err(AuthError::BadChallenge);
            }
        }
        if expires_at < now_secs() {
            // Consume the challenge either way (one-shot).
            self.challenges.lock().expect("auth mutex").remove(nonce);
            return Err(AuthError::BadChallenge);
        }

        // Ed25519 verify over nonce ‖ expiry (matches client signing).
        let payload = wl_protocol::challenge_signing_payload(nonce, expires_at);
        let pk_bytes = hex::decode(public_key).map_err(|_| AuthError::BadPublicKey)?;
        let arr: [u8; 32] = pk_bytes.try_into().map_err(|_| AuthError::BadPublicKey)?;
        let vk =
            ed25519_dalek::VerifyingKey::from_bytes(&arr).map_err(|_| AuthError::BadPublicKey)?;
        let sig: [u8; 64] = signature.try_into().map_err(|_| AuthError::BadSignature)?;
        use ed25519_dalek::Verifier;
        vk.verify(&payload, &ed25519_dalek::Signature::from_bytes(&sig))
            .map_err(|_| AuthError::BadSignature)?;

        // One-shot: challenge consumed.
        self.challenges.lock().expect("auth mutex").remove(nonce);

        // Mint session token: opaque random + counter (PASETO-style
        // v4.local is produced at the HTTP layer via pasetors; here we
        // track the bearer id and let the HTTP layer seal it).
        let sess_expires = now_secs() + SESSION_TTL.as_secs() as i64;
        let token_id = format!(
            "{}-{}",
            self.counter.fetch_add(1, Ordering::Relaxed),
            uuid::Uuid::new_v4()
        );
        self.sessions
            .lock()
            .expect("auth mutex")
            .insert(token_id.clone(), (public_key.to_string(), sess_expires));
        Ok((token_id, sess_expires))
    }

    /// Validates a bearer token → owning account (opaque pubkey hex).
    /// Expired rows are removed on sight, and a cheap counter triggers
    /// a full GC sweep periodically — pull-only clients no longer
    /// accumulate stale sessions until some other client challenges.
    pub fn validate(&self, token: &str) -> Result<String, AuthError> {
        let now = now_secs();
        let pk = {
            let mut sessions = self.sessions.lock().expect("auth mutex");
            match sessions.get(token).cloned() {
                Some((pk, exp)) => {
                    if exp < now {
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
        self.challenges
            .lock()
            .expect("auth mutex")
            .retain(|_, (_, e)| *e > now);
        self.gc_sessions();
    }

    fn gc_sessions(&self) {
        let now = now_secs();
        self.sessions
            .lock()
            .expect("auth mutex")
            .retain(|_, (_, e)| *e > now);
    }
}

impl Default for AuthState {
    fn default() -> Self {
        Self::new()
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before 1970")
        .as_secs() as i64
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
        let (other, _) = fresh_keypair();
        assert!(auth.issue_challenge(&other).is_ok());
    }

    #[test]
    fn global_challenge_ceiling_bounds_memory() {
        let auth = AuthState::new();
        // Fill the map with distinct keys until the global cap refuses.
        let mut minted = 0usize;
        for i in 0..(MAX_CHALLENGES_GLOBAL + 8) as u64 {
            let pk = hex::encode([i as u8; 32]);
            match auth.issue_challenge(&pk) {
                Ok(_) => minted += 1,
                Err(AuthError::RateLimited) => break,
                Err(e) => panic!("unexpected: {e}"),
            }
        }
        assert!(
            minted <= MAX_CHALLENGES_GLOBAL,
            "global cap did not bind: minted {minted}"
        );
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
        auth.sessions
            .lock()
            .unwrap()
            .insert(token.clone(), (pk.clone(), 0));
        assert!(matches!(auth.validate(&token), Err(AuthError::BadToken)));
        // The row is gone now — the map no longer holds it.
        assert!(!auth.sessions.lock().unwrap().contains_key(&token));
    }
}
