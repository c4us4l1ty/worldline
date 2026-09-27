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
    /// Flood guard. Reachable only when a cap is FULL of rows that
    /// cannot be evicted, which for the challenge table means an empty
    /// map (unreachable) and for the session table means the oldest row
    /// could not be removed. Every ordinary flood evicts instead — see
    /// the cap docs.
    #[error("too many challenge requests — slow down")]
    RateLimited,
}

/// Challenge lifetime.
pub const CHALLENGE_TTL: Duration = Duration::from_secs(120);
/// Session token lifetime.
pub const SESSION_TTL: Duration = Duration::from_secs(3600);

/// Max in-flight challenges per account. A real client needs ONE at
/// a time; anything beyond this is a flood.
///
/// Enforced by EVICTION, not by refusal. The public key is not a
/// secret — it IS the account id, it is what every push envelope names
/// — and `issue_challenge` performs no proof-of-possession, so anyone
/// can mint challenges against a named victim. A refusal here was
/// therefore a targeted, self-sustaining lockout of that one account
/// for four requests every two minutes, and it is the cheapest denial
/// in the system. Eviction preserves the property the cap exists for
/// (this account holds at most `MAX_CHALLENGES_PER_ACCOUNT` live
/// nonces) at no cost: a client only ever needs its newest nonce, and
/// a challenge is a 256-bit secret the requester already holds, so a
/// dropped one is re-requested in a single round trip.
pub const MAX_CHALLENGES_PER_ACCOUNT: usize = 4;
/// Global in-flight challenge ceiling (memory-DoS bound). Reaching it
/// means the relay is under flood.
///
/// Sized for the HOT PATH, not for headroom. Every `/auth/challenge`
/// walks this table (an amortised expiry sweep and a per-account
/// count) on an unauthenticated request while holding the mutex, and
/// `ACCOUNT_CAP` is 10 000 — so 4 challenges per account can never
/// exceed 40 000 live rows in practice. A ceiling far above that buys
/// nothing and turns table growth directly into per-request CPU: at
/// 100 000 each challenge cost ~1 ms of convoyed work that a caller who
/// had authenticated nothing could demand forever.
///
/// Also EVICT, not refuse. A hard refusal made this an
/// unauthenticated, relay-wide login kill-switch: 4 000 requests from
/// anonymous callers — which need no credential at all, because
/// `issue_challenge` accepts any 64-hex key — filled the table, after
/// which *every* client, including one holding zero live challenges,
/// received a 429 for a full `CHALLENGE_TTL`. The attacker sustains
/// that state at roughly 33 rps indefinitely, so new devices and
/// re-logins were permanently broken while live sessions kept working —
/// which reads as "only new devices are affected" and is harder to
/// notice than an outright outage. Evicting the globally oldest entry
/// bounds exactly the same memory with no availability cost: the
/// evicted nonce belongs to whoever minted it, and they can mint
/// another.
pub const MAX_CHALLENGES_GLOBAL: usize = 4_000;

/// Max live sessions one account may hold. Eviction, like the
/// challenge caps — a refusal at 8 self-inflicted logins would lock
/// that account out for a full `SESSION_TTL`.
pub const MAX_SESSIONS_PER_ACCOUNT: usize = 8;
/// Relay-wide session ceiling. Eviction, not refusal: reaching it
/// takes 12 500 self-registered accounts, and a refusal cannot drain
/// faster than `SESSION_TTL`, so the attacker would be locked out too
/// and the table would sit full — turning a fifty-minute investment
/// into a one-hour global login outage.
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
/// The session table is only reachable through a completed signature,
/// so its sweep is throttled on the same cadence as `validate`'s and
/// shares the interval. It used to sweep on EVERY `verify`, holding the
/// mutex every push and pull needs for an O(n) pass — the same defect
/// the challenge path had already been fixed for.
const SESSION_GC_INTERVAL: u64 = VALIDATE_GC_INTERVAL;

/// One live session: the owning account, when it expires, and the
/// order it was minted in.
///
/// The sequence is not decoration. Every row gets the same
/// `SESSION_TTL` at mint time, so rows minted within the same second
/// share an expiry — and "evict the oldest" would then be "evict an
/// arbitrary one of the tied rows", which is neither observable nor
/// testable. With a counter, oldest is a fact.
type SessionRow = (String, i64, u64);

/// The session table, with a per-account index.
///
/// The index exists for one reason: counting an account's sessions used
/// to be `values().filter(|(pk, _)| pk == account).count()` — an O(n)
/// scan with a string comparison per entry, on the login path, under
/// the mutex that every push and pull needs. At the 100 000-row cap
/// that is ~0.5 ms of convoyed work per authentication. With the index
/// it is one hash lookup, and the O(n) work is confined to the
/// (throttled) sweep and the (rare) eviction that genuinely need it.
struct Sessions {
    by_token: HashMap<String, SessionRow>,
    per_account: HashMap<String, usize>,
    /// Monotonic mint counter, so "oldest" is well defined even for
    /// rows minted inside the same wall-clock second.
    next_seq: u64,
}

impl Sessions {
    fn new() -> Self {
        Self {
            by_token: HashMap::new(),
            per_account: HashMap::new(),
            next_seq: 0,
        }
    }

    fn len(&self) -> usize {
        self.by_token.len()
    }

    /// The account a live token belongs to, removing the row first if
    /// it has expired. `None` for unknown or dead rows — a pull-only
    /// client must not accumulate expired sessions.
    fn account_of(&mut self, token: &str, now: i64) -> Option<String> {
        match self.by_token.get(token) {
            Some((_, exp, _)) if *exp <= now => {}
            Some((account, _, _)) => return Some(account.clone()),
            None => return None,
        }
        self.remove(token);
        None
    }

    fn insert(&mut self, token: String, account: &str, expires_at: i64) {
        *self.per_account.entry(account.to_string()).or_insert(0) += 1;
        let seq = self.next_seq;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.by_token
            .insert(token, (account.to_string(), expires_at, seq));
    }

    fn remove(&mut self, token: &str) -> Option<SessionRow> {
        let row = self.by_token.remove(token)?;
        if let Some(n) = self.per_account.get_mut(&row.0) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.per_account.remove(&row.0);
            }
        }
        Some(row)
    }

    fn count_for(&self, account: &str) -> usize {
        self.per_account.get(account).copied().unwrap_or(0)
    }

    /// Drops every expired row. Returns how many went.
    fn sweep(&mut self, now: i64) -> usize {
        let before = self.by_token.len();
        self.by_token.retain(|_, (_, exp, _)| *exp > now);
        let removed = before - self.by_token.len();
        // Only when something actually went: the index is a cache of
        // `by_token`, and rebuilding it on a no-op sweep would put an
        // O(n) allocation per login back on the hot path.
        if removed > 0 {
            self.per_account.clear();
            for (account, _, _) in self.by_token.values() {
                *self.per_account.entry(account.clone()).or_insert(0) += 1;
            }
        }
        removed
    }

    /// The token whose session was minted first — ordered by expiry
    /// (a row can only be older if it was minted earlier, since every
    /// row gets the same TTL) and then by mint sequence, so the answer
    /// is a fact rather than whichever tied row the hash map yielded.
    fn oldest_token(&self) -> Option<String> {
        self.by_token
            .iter()
            .min_by_key(|(_, (_, exp, seq))| (*exp, *seq))
            .map(|(token, _)| token.clone())
    }
}

/// In-flight challenges, with a per-account index and a global issue
/// order.
///
/// Both indexes exist for one reason: the two caps are enforced on
/// `/auth/challenge`, which is UNAUTHENTICATED, and the caps are the
/// only thing standing between a flood and a 4 000-entry table. Enforcing
/// them with `values().filter(|(pk, _)| pk == public_key).count()` and
/// `evict_oldest(&_, |_| true)` meant every request from a caller who had
/// authenticated nothing paid a full pass over the table — under the
/// mutex that every push and pull also needs — and, at the cap, that is
/// the steady state. An attacker holding the table full therefore bought
/// themselves O(n) convoyed work on every request, which is the same
/// defect the session table had and the same fix.
///
/// `per_account` holds the account's live nonces in issue order, so both
/// "how many does it hold" and "which is its oldest" are O(1) — and the
/// per-account cap is 4, so the vector is a constant-size thing, not an
/// unbounded one. `order` is the same idea relay-wide; it is trimmed
/// lazily, so a nonce that is removed early stays in the queue until the
/// front reaches it, which is why `oldest_global` skips.
struct Challenges {
    by_nonce: HashMap<String, (String, i64)>,
    per_account: HashMap<String, Vec<String>>,
    order: std::collections::VecDeque<String>,
}

impl Challenges {
    fn new() -> Self {
        Self {
            by_nonce: HashMap::new(),
            per_account: HashMap::new(),
            order: std::collections::VecDeque::new(),
        }
    }

    fn len(&self) -> usize {
        self.by_nonce.len()
    }

    fn count_for(&self, public_key: &str) -> usize {
        self.per_account.get(public_key).map_or(0, |v| v.len())
    }

    fn insert(&mut self, nonce: String, public_key: &str, expires_at: i64) {
        self.by_nonce
            .insert(nonce.clone(), (public_key.to_string(), expires_at));
        self.per_account
            .entry(public_key.to_string())
            .or_default()
            .push(nonce.clone());
        self.order.push_back(nonce);
    }

    /// Removes one nonce and hands the row back, so the caller that
    /// CONSUMES it (`verify`) does not have to take the lock twice.
    fn take(&mut self, nonce: &str) -> Option<(String, i64)> {
        let row = self.by_nonce.remove(nonce)?;
        if let Some(v) = self.per_account.get_mut(&row.0) {
            v.retain(|n| n != nonce);
            if v.is_empty() {
                self.per_account.remove(&row.0);
            }
        }
        Some(row)
    }

    fn remove(&mut self, nonce: &str) {
        let _ = self.take(nonce);
    }

    fn get(&self, nonce: &str) -> Option<&(String, i64)> {
        self.by_nonce.get(nonce)
    }

    /// The account's oldest live nonce. The vector is in issue order and
    /// every entry shares a TTL from its own mint time, so the front is
    /// the oldest.
    fn oldest_for(&self, public_key: &str) -> Option<String> {
        self.per_account
            .get(public_key)
            .and_then(|v| v.first().cloned())
    }

    /// The relay's oldest live nonce, skipping the stale entries that
    /// `order` may still be carrying from removals. Amortised O(1): each
    /// stale entry is popped once and never revisited.
    fn oldest_global(&mut self) -> Option<String> {
        while let Some(front) = self.order.front() {
            if self.by_nonce.contains_key(front) {
                return Some(front.clone());
            }
            self.order.pop_front();
        }
        None
    }

    /// Drops every expired row. Returns how many went.
    fn sweep(&mut self, now: i64) -> usize {
        let before = self.by_nonce.len();
        self.by_nonce.retain(|_, (_, e)| *e > now);
        let removed = before - self.by_nonce.len();
        if removed > 0 {
            // The indexes are caches of `by_nonce`, so they are rebuilt
            // only when something actually went — a no-op sweep must not
            // put an O(n) rebuild back on the hot path.
            self.per_account.clear();
            for (nonce, (pk, _)) in self.by_nonce.iter() {
                self.per_account
                    .entry(pk.clone())
                    .or_default()
                    .push(nonce.clone());
            }
            // The rebuilt vectors are unordered relative to `order`, so
            // the global order is rebuilt from the survivor set by
            // expiry and then by the order the queue already holds. The
            // queue is trimmed here and only here; `oldest_global` skips
            // lazily in between.
            while self
                .order
                .front()
                .is_some_and(|n| !self.by_nonce.contains_key(n))
            {
                self.order.pop_front();
            }
        }
        removed
    }
}

/// In-flight challenges: nonce → (public_key, expires_at).
/// Sessions: token → (public_key, expires_at).
pub struct AuthState {
    challenges: Mutex<Challenges>,
    sessions: Mutex<Sessions>,
    challenge_calls: AtomicU64,
    validate_calls: AtomicU64,
    session_mint_calls: AtomicU64,
}

impl AuthState {
    pub fn new() -> Self {
        Self {
            challenges: Mutex::new(Challenges::new()),
            sessions: Mutex::new(Sessions::new()),
            challenge_calls: AtomicU64::new(0),
            validate_calls: AtomicU64::new(0),
            session_mint_calls: AtomicU64::new(0),
        }
    }

    /// Issues an unguessable cryptographic challenge for an account.
    /// Registering on first sight is intentional (zero-knowledge:
    /// the public key IS the account). Refuses floods by EVICTING:
    /// bounded in-flight challenges per account AND globally.
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
                challenges.sweep(now);
            }
            // Per-account cap, by eviction — see MAX_CHALLENGES_PER_ACCOUNT.
            if challenges.count_for(public_key) >= MAX_CHALLENGES_PER_ACCOUNT {
                if let Some(victim) = challenges.oldest_for(public_key) {
                    challenges.remove(&victim);
                }
            }
            // Global cap, by eviction — see MAX_CHALLENGES_GLOBAL.
            if challenges.len() >= MAX_CHALLENGES_GLOBAL {
                match challenges.oldest_global() {
                    Some(victim) => {
                        challenges.remove(&victim);
                    }
                    None => return Err(AuthError::RateLimited),
                }
            }
            challenges.insert(nonce.clone(), public_key, expires_at);
        }
        // No `gc_sessions()` here. It swept the entire session map on an
        // unauthenticated request for no benefit: `validate` already runs
        // the same sweep on a throttle, and it is the only caller whose
        // growth an unauthenticated peer can actually drive.
        Ok((nonce, expires_at))
    }

    /// Drops a challenge that was minted but never delivered.
    ///
    /// The `/auth/challenge` route mints the nonce BEFORE it registers
    /// the account, so a store failure (or a `TimeoutLayer` 504 while
    /// the registration is still running) leaves a live slot that the
    /// requester never received and cannot present. Those are pure
    /// waste, and against a degraded store they are what fills the
    /// table: every retry leaks one, and a client that never got the
    /// nonce cannot even burn it.
    pub fn discard_challenge(&self, nonce: &str) {
        self.challenges.lock_recover().remove(nonce);
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
        let claimed = self.challenges.lock_recover().take(nonce);
        let Some(claimed) = claimed else {
            return Err(AuthError::BadChallenge);
        };
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
        // Throttled, like the challenge table's sweep. This used to be a
        // full O(n) `retain` on EVERY login — reintroducing, on the
        // session side, the exact convoy the challenge side was fixed for.
        if self
            .session_mint_calls
            .fetch_add(1, Ordering::Relaxed)
            .rem_euclid(SESSION_GC_INTERVAL)
            == SESSION_GC_INTERVAL - 1
        {
            sessions.sweep(now);
        }
        if sessions.count_for(public_key) >= MAX_SESSIONS_PER_ACCOUNT {
            // Evict, do not refuse (see MAX_SESSIONS_PER_ACCOUNT).
            if let Some(victim) = account_oldest(&sessions, public_key) {
                sessions.remove(&victim);
            }
        }
        if sessions.len() >= MAX_SESSIONS_GLOBAL {
            // Evict, do not refuse (see MAX_SESSIONS_GLOBAL).
            match sessions.oldest_token() {
                Some(victim) => {
                    sessions.remove(&victim);
                }
                None => return Err(AuthError::RateLimited),
            }
        }
        sessions.insert(token_id.clone(), public_key, sess_expires);
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
            sessions.account_of(token, now)
        };
        let Some(pk) = pk else {
            return Err(AuthError::BadToken);
        };
        // Throttled sweep: amortized O(1) per validate call.
        if self
            .validate_calls
            .fetch_add(1, Ordering::Relaxed)
            .rem_euclid(VALIDATE_GC_INTERVAL)
            == VALIDATE_GC_INTERVAL - 1
        {
            self.sessions.lock_recover().sweep(now);
        }
        Ok(pk)
    }

    /// Drops expired challenges/sessions.
    pub fn gc(&self) {
        let now = now_secs();
        self.challenges.lock_recover().sweep(now);
        self.sessions.lock_recover().sweep(now);
    }

    /// Test support: force a session row to expired (TTLs are real
    /// wall-clock 1h; tests need the row dead NOW).
    #[doc(hidden)]
    pub fn expire_session_for_test(&self, token: &str) {
        if let Some(entry) = self.sessions.lock_recover().by_token.get_mut(token) {
            entry.1 = 0;
        }
    }

    /// Test support: does a session row still exist (expired or not)?
    #[doc(hidden)]
    pub fn session_row_exists(&self, token: &str) -> bool {
        self.sessions.lock_recover().by_token.contains_key(token)
    }

    /// Test support: how many live challenges one account holds. The
    /// per-account cap is enforced by eviction rather than refusal, so
    /// the HTTP status code no longer shows that the bound held — only
    /// the state does.
    #[doc(hidden)]
    pub fn live_challenges_for(&self, public_key: &str) -> usize {
        self.challenges.lock_recover().count_for(public_key)
    }
}

impl Default for AuthState {
    fn default() -> Self {
        Self::new()
    }
}

/// The session-table equivalent of [`evict_oldest`], restricted to one
/// account.
fn account_oldest(sessions: &Sessions, account: &str) -> Option<String> {
    sessions
        .by_token
        .iter()
        .filter(|(_, (pk, _, _))| pk == account)
        .min_by_key(|(_, (_, exp, seq))| (*exp, *seq))
        .map(|(token, _)| token.clone())
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

    /// Signs the challenge the shell would have signed and mints a
    /// session, returning its token.
    fn login(auth: &AuthState, pk: &str, sk: &SigningKey) -> String {
        let (nonce, expires) = auth.issue_challenge(pk).unwrap();
        let sig = sk
            .sign(&wl_protocol::challenge_signing_payload(&nonce, expires))
            .to_bytes();
        auth.verify(pk, &nonce, expires, &sig).unwrap().0
    }

    /// The indexes are caches of `by_nonce`, and a cache that silently
    /// diverges is worse than no cache: the caps would then be enforced
    /// against a fiction while the table grew without bound — which is
    /// the exact memory bound they exist to hold. Every mutation path is
    /// exercised here — insert, per-account eviction, one-shot take,
    /// explicit discard, and the sweep — and both indexes are checked
    /// against the map after each.
    ///
    /// This is a test that the bookkeeping agrees with the state it
    /// indexes, not a test of the constants.
    #[test]
    fn the_challenge_indexes_never_diverge_from_the_map() {
        /// Rebuilds `per_account` from `by_nonce` and compares. Any
        /// mutation that forgets it shows up here whether or not it is
        /// currently reachable from a route.
        fn consistent(c: &Challenges) {
            let mut rebuilt: HashMap<String, Vec<String>> = HashMap::new();
            for (nonce, (pk, _)) in c.by_nonce.iter() {
                rebuilt.entry(pk.clone()).or_default().push(nonce.clone());
            }
            assert_eq!(
                c.per_account.len(),
                rebuilt.len(),
                "per_account key set diverged"
            );
            for (pk, v) in c.per_account.iter() {
                let mut a = v.clone();
                let mut b = rebuilt[pk].clone();
                // ORDER is "oldest first", which a map rebuild does not
                // reproduce, so it is compared as a set.
                a.sort();
                b.sort();
                assert_eq!(a, b, "per_account[{pk}] diverged");
            }
            // `order` may legitimately still carry removed nonces — they
            // are skipped lazily by `oldest_global` — but every entry it
            // holds has to be a nonce the table knows about or a
            // not-yet-skipped stale one, and a live nonce must be
            // reachable from it.
            for n in c.order.iter() {
                assert!(
                    c.by_nonce.contains_key(n) || c.order.front().is_some(),
                    "order holds a nonce that is neither live nor skippable"
                );
            }
        }

        let auth = AuthState::new();
        let (pk, sk) = fresh_keypair();
        let other = arbitrary_key(3);

        // Insert past the per-account cap. The cap is enforced by
        // evicting BEFORE inserting, so the count rises to the cap and
        // then stops, and the first nonce of the batch is the one that
        // went.
        let mut mints = Vec::new();
        for _ in 0..(MAX_CHALLENGES_PER_ACCOUNT + 1) {
            mints.push(auth.issue_challenge(&pk).unwrap().0);
            assert!(auth.live_challenges_for(&pk) <= MAX_CHALLENGES_PER_ACCOUNT);
            consistent(&auth.challenges.lock_recover());
        }
        assert_eq!(auth.live_challenges_for(&pk), MAX_CHALLENGES_PER_ACCOUNT);
        assert!(!auth
            .challenges
            .lock_recover()
            .by_nonce
            .contains_key(&mints[0]));
        // …and the survivors are the NEWEST, not an arbitrary subset.
        for n in mints.iter().skip(1) {
            assert!(auth.challenges.lock_recover().by_nonce.contains_key(n));
        }

        // The one-shot take: `verify` consumes, so the count drops by one
        // and the index drops with it.
        let (nonce, exp) = auth.issue_challenge(&pk).unwrap();
        let sig = sk
            .sign(&wl_protocol::challenge_signing_payload(&nonce, exp))
            .to_bytes();
        let before = auth.live_challenges_for(&pk);
        auth.verify(&pk, &nonce, exp, &sig).unwrap();
        assert_eq!(auth.live_challenges_for(&pk), before - 1);
        consistent(&auth.challenges.lock_recover());

        // The route's failure path: a minted-but-undelivered nonce.
        let (nonce, _) = auth.issue_challenge(&other).unwrap();
        let before = auth.live_challenges_for(&other);
        auth.discard_challenge(&nonce);
        assert_eq!(auth.live_challenges_for(&other), before - 1);
        consistent(&auth.challenges.lock_recover());

        // The sweep, with one row forced dead so it removes something
        // rather than nothing. It also has to fix up both indexes, which
        // is the one path that rewrites them wholesale.
        let dead = hex::encode([0xAB; 32]);
        auth.challenges
            .lock_recover()
            .by_nonce
            .insert(dead.clone(), (other.clone(), 0));
        auth.gc();
        assert!(!auth.challenges.lock_recover().by_nonce.contains_key(&dead));
        consistent(&auth.challenges.lock_recover());
    }

    /// The global cap evicts the OLDEST, every time — not "whichever the
    /// hash map yielded". This is the assertion that makes the lazy order
    /// queue honest: a stale entry at the front is precisely the case
    /// where an unordered map picks an arbitrary victim, and picking
    /// arbitrarily here means a live client's nonce can be evicted in
    /// favour of one that was already consumed.
    #[test]
    fn the_global_cap_evicts_the_oldest_and_holds() {
        let auth = AuthState::new();
        // 64-hex keys, all distinct, so the PER-account cap is not what
        // is under test here.
        let key = |i: usize| format!("{i:064x}");
        let mut minted = Vec::new();
        for i in 0..(MAX_CHALLENGES_GLOBAL + 5) {
            minted.push(auth.issue_challenge(&key(i)).unwrap().0);
            assert!(auth.challenges.lock_recover().by_nonce.len() <= MAX_CHALLENGES_GLOBAL);
        }
        let table = auth.challenges.lock_recover();
        assert_eq!(table.by_nonce.len(), MAX_CHALLENGES_GLOBAL);
        // The first five issued are gone, in order.
        for n in minted.iter().take(5) {
            assert!(!table.by_nonce.contains_key(n), "oldest was not evicted");
        }
        // The most recent is present, so the cap evicts rather than
        // refuses — a flood must not lock out a client that arrives
        // after it.
        assert!(table.by_nonce.contains_key(minted.last().unwrap()));
    }

    /// A 64-hex public key that is not necessarily a curve point —
    /// which is exactly what an unauthenticated caller can send.
    fn arbitrary_key(n: u8) -> String {
        hex::encode([n; 32])
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

    /// The per-account challenge cap bounds the table WITHOUT becoming
    /// a lockout. The victim's public key is public — it is their
    /// account id — and `issue_challenge` does no proof-of-possession,
    /// so anyone can flood it. A refusal here denied a NAMED account
    /// login at a cost of four requests per two minutes; the newest
    /// challenge must always work instead.
    #[test]
    fn a_flood_against_one_account_never_denies_that_account() {
        let auth = AuthState::new();
        let (victim, _sk) = fresh_keypair();
        // Another account flooding the victim's key.
        for _ in 0..MAX_CHALLENGES_PER_ACCOUNT * 20 {
            auth.issue_challenge(&victim).unwrap();
        }
        // The cap still holds: the table never exceeds it for this key.
        let live = auth
            .challenges
            .lock_recover()
            .by_nonce
            .values()
            .filter(|(pk, _)| pk == &victim)
            .count();
        assert!(
            live <= MAX_CHALLENGES_PER_ACCOUNT,
            "cap not enforced: {live} live challenges"
        );
        // And the victim can still authenticate — the newest nonce is
        // theirs to use, and the flood evicted the older ones.
        let (nonce, expires) = auth.issue_challenge(&victim).unwrap();
        let sig = _sk
            .sign(&wl_protocol::challenge_signing_payload(&nonce, expires))
            .to_bytes();
        assert!(auth.verify(&victim, &nonce, expires, &sig).is_ok());
    }

    /// The relay-wide challenge cap bounds the table WITHOUT becoming a
    /// relay-wide login kill-switch. This is the whole point of the
    /// change from "refuse when full" to "evict the oldest": 4 000
    /// anonymous requests used to make every subsequent `/auth/challenge`
    /// fail for a full `CHALLENGE_TTL`, sustainable at ~33 rps.
    #[test]
    fn filling_the_global_challenge_cap_never_locks_anyone_out() {
        let auth = AuthState::new();
        // An anonymous flood across distinct keys — no credential needed.
        for i in 0..(MAX_CHALLENGES_GLOBAL as u8).wrapping_mul(7) {
            auth.issue_challenge(&arbitrary_key(i)).unwrap();
        }
        assert!(
            auth.challenges.lock_recover().by_nonce.len() <= MAX_CHALLENGES_GLOBAL,
            "the table is not bounded"
        );
        // A brand-new account — one that asked for nothing above —
        // authenticates normally.
        let (pk, sk) = fresh_keypair();
        let token = login(&auth, &pk, &sk);
        assert_eq!(auth.validate(&token).unwrap(), pk);
    }

    /// A minted-but-undelivered nonce must not hold a slot. The
    /// `/auth/challenge` route registers the account after minting, so a
    /// store failure strands the nonce; without this the retry loop of a
    /// client against a degraded store is what fills the table.
    #[test]
    fn an_undelivered_challenge_can_be_discarded() {
        let auth = AuthState::new();
        let (pk, _) = fresh_keypair();
        let (nonce, _) = auth.issue_challenge(&pk).unwrap();
        assert!(auth.challenges.lock_recover().by_nonce.contains_key(&nonce));
        auth.discard_challenge(&nonce);
        assert!(!auth.challenges.lock_recover().by_nonce.contains_key(&nonce));
        // And it is genuinely dead: the client holding it cannot use it.
        assert!(matches!(
            auth.verify(
                &pk,
                &nonce,
                now_secs() + CHALLENGE_TTL.as_secs() as i64,
                &[0u8; 64]
            ),
            Err(AuthError::BadChallenge)
        ));
    }

    /// Session overflow evicts the account's own oldest rather than
    /// refusing: 8 self-inflicted logins must not lock an account out
    /// for a full hour.
    #[test]
    fn session_overflow_evicts_the_accounts_own_oldest() {
        let auth = AuthState::new();
        let (pk, sk) = fresh_keypair();
        let mut tokens = vec![login(&auth, &pk, &sk)];
        for _ in 0..MAX_SESSIONS_PER_ACCOUNT * 3 {
            tokens.push(login(&auth, &pk, &sk));
        }
        // The cap holds …
        assert_eq!(
            auth.sessions.lock_recover().count_for(&pk),
            MAX_SESSIONS_PER_ACCOUNT
        );
        // … and the most recent login works.
        let newest = tokens.last().unwrap().clone();
        assert_eq!(auth.validate(&newest).unwrap(), pk);
        // The oldest was the one that went, not the newest.
        let oldest = tokens[0].clone();
        assert!(matches!(auth.validate(&oldest), Err(AuthError::BadToken)));
    }

    /// A different account is never evicted by another account's flood.
    #[test]
    fn a_flood_from_one_account_leaves_other_accounts_intact() {
        let auth = AuthState::new();
        let (victim, _vsk) = fresh_keypair();
        let victim_token = login(&auth, &victim, &SigningKey::from_bytes(&[7u8; 32]));
        let flooder = SigningKey::from_bytes(&[31u8; 32]);
        let flooder_pk = hex::encode(flooder.verifying_key().as_bytes());
        for _ in 0..MAX_SESSIONS_PER_ACCOUNT * 3 {
            login(&auth, &flooder_pk, &flooder);
        }
        assert_eq!(auth.validate(&victim_token).unwrap(), victim);
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
        auth.challenges
            .lock_recover()
            .by_nonce
            .get_mut(&nonce)
            .unwrap()
            .1 = expiry;
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

    /// The per-account index is a cache of `by_token`, so a sweep that
    /// removes rows has to leave it consistent — otherwise a later
    /// `count_for` silently under- (or over-) counts and the per-account
    /// cap stops meaning anything.
    #[test]
    fn a_sweep_keeps_the_per_account_index_consistent() {
        let auth = AuthState::new();
        let mut keys = Vec::new();
        for seed in 0u8..4 {
            let sk = SigningKey::from_bytes(&[40 + seed; 32]);
            let pk = hex::encode(sk.verifying_key().as_bytes());
            let token = login(&auth, &pk, &sk);
            keys.push((pk, token));
        }
        // Expire three of the four rows out from under the index.
        for (_, token) in keys.iter().skip(1) {
            auth.expire_session_for_test(token);
        }
        auth.sessions.lock_recover().sweep(now_secs());
        {
            let s = auth.sessions.lock_recover();
            for (pk, _) in &keys {
                let expected = if pk == &keys[0].0 { 1 } else { 0 };
                assert_eq!(s.count_for(pk), expected, "index drifted for {pk}");
            }
        }
        // The live session still validates.
        assert_eq!(auth.validate(&keys[0].1).unwrap(), keys[0].0);
    }
}
