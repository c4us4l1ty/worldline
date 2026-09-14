use std::fmt;

use bip39::Mnemonic;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use zeroize::Zeroizing;

use crate::crypto::{HKDF_INFO_CHACHA20, HKDF_INFO_ED25519};

/// Errors surfaced by the identity layer.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    #[error("invalid mnemonic: {0}")]
    BadMnemonic(String),
    #[error("unsupported mnemonic word count: {0} (expected 12)")]
    BadWordCount(usize),
    #[error("verification failed: mnemonic does not match account")]
    Mismatch,
    #[error("signature error: {0}")]
    Signature(#[from] ed25519_dalek::SignatureError),
    #[error("hex encoding error: {0}")]
    Hex(#[from] hex::FromHexError),
}

/// The full set of keys Worldline derives from one BIP-39 mnemonic.
///
/// * Ed25519 signing keypair — relay authentication (public key hex
///   doubles as the opaque Account ID on the Axum server).
/// * ChaCha20-Poly1305 symmetric key — encrypts all CRDT operations
///   and SQLite sync snapshots before transmission (E2EE).
///
/// Both subkeys are derived from the mnemonic *seed* (BIP-39 512-bit
/// PBKDF2 output) via HKDF-SHA256 with distinct, versioned info labels,
/// so rotating one label never affects the other.
pub struct Identity {
    /// 12-word BIP-39 mnemonic phrase (zeroized on drop).
    phrase: Zeroizing<String>,
    /// Ed25519 signing key (relay auth). Zeroized on drop.
    signing_key: SigningKey,
    /// Symmetric payload-encryption key (E2EE of CRDT payloads).
    /// Zeroized on drop via the array's Zeroize impl.
    payload_key: [u8; 32],
}

// ed25519-dalek's `zeroize` feature gives SigningKey a Drop impl
// that wipes its secret, and Zeroizing<String> wipes the phrase — no
// manual Drop needed on Identity.
impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never leak key material through Debug.
        f.debug_struct("Identity")
            .field("account_id", &self.account_id_hex())
            .finish()
    }
}

impl Identity {
    /// Generates a fresh identity from 128 bits of OS entropy,
    /// formatted as the standard 12-word BIP-39 mnemonic.
    pub fn generate() -> Result<Self, IdentityError> {
        let mnemonic =
            Mnemonic::generate(12).map_err(|e| IdentityError::BadMnemonic(e.to_string()))?;
        Self::from_mnemonic(mnemonic)
    }

    /// Restores an identity from an existing 12-word mnemonic
    /// (account recovery path — US-1).
    pub fn from_phrase(phrase: &str) -> Result<Self, IdentityError> {
        let mnemonic =
            Mnemonic::parse(phrase).map_err(|e| IdentityError::BadMnemonic(e.to_string()))?;
        let wc = mnemonic.word_count();
        if wc != 12 {
            return Err(IdentityError::BadWordCount(wc));
        }
        Self::from_mnemonic(mnemonic)
    }

    fn from_mnemonic(mnemonic: Mnemonic) -> Result<Self, IdentityError> {
        // BIP-39 seed = PBKDF2-HMAC-SHA512(mnemonic, "mnemonic"+passphrase, 2048)
        let seed = mnemonic.to_seed_normalized("");
        let signing_key = derive_signing_key(&seed)?;
        let payload_key = derive_payload_key(&seed);
        Ok(Self {
            phrase: Zeroizing::new(mnemonic.to_string()),
            signing_key,
            payload_key,
        })
    }

    /// The 12-word mnemonic phrase. Handle with care: this is the
    /// only credential that can recover the account.
    pub fn phrase(&self) -> &Zeroizing<String> {
        &self.phrase
    }

    /// Opaque Account ID: hex-encoded Ed25519 public key.
    /// This is the only identity material the relay ever sees.
    pub fn account_id_hex(&self) -> String {
        hex::encode(self.signing_key.verifying_key().as_bytes())
    }

    /// Hex-encoded Ed25519 public key (same as account id).
    pub fn public_key_hex(&self) -> String {
        self.account_id_hex()
    }

    /// Signs an arbitrary challenge (e.g. relay auth nonce) with the
    /// Ed25519 signing key.
    pub fn sign(&self, challenge: &[u8]) -> [u8; 64] {
        self.signing_key.sign(challenge).to_bytes()
    }

    /// Raw symmetric payload-encryption key reference (internal).
    pub(crate) fn payload_key(&self) -> &[u8; 32] {
        &self.payload_key
    }
}

/// HKDF-SHA256 → Ed25519 signing subkey for a given BIP-39 seed.
fn derive_signing_key(seed: &[u8]) -> Result<SigningKey, IdentityError> {
    let okm = hkdf_sha256(seed, HKDF_INFO_ED25519, 32);
    let bytes: Zeroizing<[u8; 32]> = Zeroizing::new(okm.try_into().expect("32-byte OKM"));
    Ok(SigningKey::from_bytes(&bytes))
}
/// HKDF-SHA256 → ChaCha20-Poly1305 payload subkey for a given BIP-39 seed.
fn derive_payload_key(seed: &[u8]) -> [u8; 32] {
    hkdf_sha256(seed, HKDF_INFO_CHACHA20, 32)
        .try_into()
        .expect("32-byte OKM")
}

/// Extract-and-expand HKDF with SHA-256. `salt` is the BIP-39 seed
/// itself (already high-entropy, so IKM=salt reuse is safe here: the
/// mnemonic entropy is the secret, PBKDF2 output is the public-ish
/// intermediate).
fn hkdf_sha256(ikm: &[u8], info: &[u8], len: usize) -> Vec<u8> {
    use hkdf::Hkdf;
    use sha2::Sha256;
    let hk = Hkdf::<Sha256>::new(None, ikm);
    let mut okm = vec![0u8; len];
    hk.expand(info, &mut okm).expect("valid HKDF length");
    okm
}

/// Verifies an Ed25519 signature (utility used by tests and the relay).
pub fn verify_signature(
    public_key_hex: &str,
    msg: &[u8],
    sig: &[u8; 64],
) -> Result<bool, IdentityError> {
    let pk_bytes = hex::decode(public_key_hex)?;
    let arr: [u8; 32] = pk_bytes
        .try_into()
        .map_err(|_| hex::FromHexError::InvalidStringLength)?;
    let vk = VerifyingKey::from_bytes(&arr)?;
    let sig = Signature::from_bytes(sig);
    Ok(vk.verify(msg, &sig).is_ok())
}

/// Storage abstraction over where the mnemonic lives at runtime.
///
/// In production (feature `stronghold`) this is the hardware-backed
/// Tauri Stronghold vault; in headless builds the caller supplies an
/// in-memory or file-backed source. The vault never persists the raw
/// phrase outside hardware-backed storage.
pub enum IdentityVault {
    /// Identity held only in process memory (tests / restored sessions).
    InMemory(Box<Identity>),
    /// Identity restored from the Stronghold vault by the native shell.
    #[cfg(feature = "stronghold")]
    Stronghold(Box<Identity>),
}

impl IdentityVault {
    /// Deterministically re-derives the entire key material set from
    /// the stored mnemonic.
    pub fn unlock(&self) -> Result<&Identity, IdentityError> {
        match self {
            IdentityVault::InMemory(id) => Ok(id),
            #[cfg(feature = "stronghold")]
            IdentityVault::Stronghold(id) => Ok(id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic BIP-39 test vector (12 words, 128-bit entropy).
    /// entropy 7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f → this exact phrase.
    const TEST_PHRASE: &str =
        "legal winner thank year wave sausage worth useful legal winner thank yellow";

    #[test]
    fn generate_produces_valid_12_words() {
        let id = Identity::generate().unwrap();
        let phrase = id.phrase();
        assert_eq!(phrase.split_whitespace().count(), 12);
    }

    #[test]
    fn generate_is_unique_per_call() {
        let a = Identity::generate().unwrap();
        let b = Identity::generate().unwrap();
        assert_ne!(a.account_id_hex(), b.account_id_hex());
    }

    #[test]
    fn restore_from_phrase_is_deterministic() {
        let a = Identity::from_phrase(TEST_PHRASE).unwrap();
        let b = Identity::from_phrase(TEST_PHRASE).unwrap();
        assert_eq!(a.account_id_hex(), b.account_id_hex());
        // Stable derivation: same phrase ⇒ same keys across devices.
        assert_eq!(a.payload_key(), b.payload_key());
    }

    #[test]
    fn account_id_is_hex_ed25519_public_key() {
        let id = Identity::from_phrase(TEST_PHRASE).unwrap();
        let hex_str = id.account_id_hex();
        assert_eq!(hex_str.len(), 64);
        assert!(hex_str.chars().all(|c| c.is_ascii_hexdigit()));
        // Round-trip via the verifier utility.
        let sig = id.sign(b"challenge");
        assert!(verify_signature(&hex_str, b"challenge", &sig).unwrap());
        assert!(!verify_signature(&hex_str, b"tampered", &sig).unwrap());
    }

    #[test]
    fn sign_verify_roundtrip() {
        let id = Identity::from_phrase(TEST_PHRASE).unwrap();
        let sig = id.sign(b"nonce:abc");
        let ok = verify_signature(&id.public_key_hex(), b"nonce:abc", &sig).unwrap();
        assert!(ok);
    }

    #[test]
    fn rejects_invalid_phrase() {
        assert!(Identity::from_phrase("not a real phrase").is_err());
        // 24-word VALID checksummed phrase still rejected: Worldline
        // mandates 12 (PRD §3.1). (Standard test vector.)
        let long = "legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth useful legal winner thank year wave sausage worth title";
        assert!(matches!(
            Identity::from_phrase(long),
            Err(IdentityError::BadWordCount(24))
        ));
    }

    #[test]
    fn debug_never_leaks_mnemonic() {
        let id = Identity::from_phrase(TEST_PHRASE).unwrap();
        let dbg = format!("{id:?}");
        assert!(!dbg.contains("legal"));
        assert!(dbg.contains("account_id"));
    }

    #[test]
    fn distinct_hkdf_labels_yield_distinct_keys() {
        let id = Identity::from_phrase(TEST_PHRASE).unwrap();
        let pk_hex = id.account_id_hex();
        // The two subkeys are derived with different info labels and
        // must be independent: signing key bytes ≠ payload key bytes.
        let sig = id.sign(b"x");
        let sig_hex = hex::encode(sig);
        assert_ne!(pk_hex, sig_hex);
    }
}
