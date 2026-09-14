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
}

/// Challenge lifetime.
pub const CHALLENGE_TTL: Duration = Duration::from_secs(120);
/// Session token lifetime.
pub const SESSION_TTL: Duration = Duration::from_secs(3600);

/// In-flight challenges: nonce → (public_key, expires_at).
/// Sessions: token → (public_key, expires_at).
pub struct AuthState {
    challenges: Mutex<HashMap<String, (String, i64)>>,
    sessions: Mutex<HashMap<String, (String, i64)>>,
    /// Monotonic counter for session token uniqueness.
    counter: AtomicU64,
}

impl AuthState {
    pub fn new() -> Self {
        Self {
            challenges: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            counter: AtomicU64::new(0),
        }
    }

    /// Issues an unguessable cryptographic challenge for an account.
    /// Registering on first sight is intentional (zero-knowledge:
    /// the public key IS the account).
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
        self.challenges
            .lock()
            .expect("auth mutex")
            .insert(nonce.clone(), (public_key.to_string(), expires_at));
        self.gc();
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
    pub fn validate(&self, token: &str) -> Result<String, AuthError> {
        let sessions = self.sessions.lock().expect("auth mutex");
        let (pk, exp) = sessions.get(token).ok_or(AuthError::BadToken)?;
        if *exp < now_secs() {
            return Err(AuthError::BadToken); // caller may GC
        }
        Ok(pk.clone())
    }

    /// Drops expired challenges/sessions (call periodically).
    pub fn gc(&self) {
        let now = now_secs();
        self.challenges
            .lock()
            .expect("auth mutex")
            .retain(|_, (_, e)| *e > now);
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
}
