use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
    ChaCha20Poly1305, Key, Nonce,
};
use zeroize::Zeroizing;

use crate::crypto::identity::Identity;

/// AEAD failures (encrypt/decrypt/nonce handling).
#[derive(Debug, thiserror::Error)]
pub enum AeadError {
    #[error("encryption failed")]
    Encrypt,
    #[error("decryption failed: payload corrupt, key mismatch, or tampered")]
    Decrypt,
    #[error("invalid nonce length: expected 12 bytes, got {0}")]
    BadNonce(usize),
}

/// 12-byte ChaCha20-Poly1305 nonce, freshly random per message.
pub type NonceBytes = [u8; 12];

/// Ciphertext envelope: `nonce ‖ tag‖ciphertext` — self-contained
/// blob ready for the CRDT outbox / relay push.
#[derive(Clone)]
pub struct Sealed {
    pub nonce: NonceBytes,
    pub ciphertext: Vec<u8>,
}

impl Sealed {
    /// Wire format: 12-byte nonce prepended to the ciphertext body.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + self.ciphertext.len());
        out.extend_from_slice(&self.nonce);
        out.extend_from_slice(&self.ciphertext);
        out
    }

    /// Parses the wire format produced by [`Sealed::to_bytes`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, AeadError> {
        if bytes.len() < 12 + 16 {
            return Err(AeadError::Decrypt);
        }
        let (nonce, ciphertext) = bytes.split_at(12);
        Ok(Self {
            nonce: nonce.try_into().expect("12-byte prefix"),
            ciphertext: ciphertext.to_vec(),
        })
    }
}

/// Encrypts an arbitrary serialisable payload with the identity's
/// ChaCha20-Poly1305 key (fresh random nonce). Associated data (e.g.
/// `table:record` routing header) is authenticated but not encrypted.
pub fn seal(identity: &Identity, plaintext: &[u8], aad: &[u8]) -> Result<Sealed, AeadError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(identity.payload_key()));
    let nonce_bytes = NonceBytes::from(ChaCha20Poly1305::generate_nonce(&mut OsRng));
    let nonce = Nonce::from(nonce_bytes);
    let sealed = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| AeadError::Encrypt)?;
    Ok(Sealed {
        nonce: nonce_bytes,
        ciphertext: sealed,
    })
}

/// Decrypts a [`Sealed`] envelope. Fails closed on any tampering,
/// key mismatch, or AAD mismatch ( ChaCha20-Poly1305 is a single
/// combined authentication pass — no unauthenticated plaintext).
pub fn unseal(
    identity: &Identity,
    sealed: &Sealed,
    aad: &[u8],
) -> Result<Zeroizing<Vec<u8>>, AeadError> {
    let cipher = ChaCha20Poly1305::new(Key::from_slice(identity.payload_key()));
    let pt = cipher
        .decrypt(
            &Nonce::from(sealed.nonce),
            Payload {
                msg: &sealed.ciphertext,
                aad,
            },
        )
        .map_err(|_| AeadError::Decrypt)?;
    Ok(Zeroizing::new(pt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::identity::Identity;

    const PHRASE: &str =
        "legal winner thank year wave sausage worth useful legal winner thank yellow";

    #[test]
    fn seal_unseal_roundtrip_with_aad() {
        let id = Identity::from_phrase(PHRASE).unwrap();
        let msg = br#"{"op":"upsert","table":"directives","id":"d-1"}"#;
        let aad = b"directives:d-1";
        let sealed = seal(&id, msg, aad).unwrap();
        let opened = unseal(&id, &sealed, aad).unwrap();
        assert_eq!(opened.as_slice(), msg);
    }

    #[test]
    fn fresh_nonce_every_message() {
        let id = Identity::from_phrase(PHRASE).unwrap();
        let a = seal(&id, b"same message", b"aad").unwrap();
        let b = seal(&id, b"same message", b"aad").unwrap();
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn tampered_ciphertext_fails_closed() {
        let id = Identity::from_phrase(PHRASE).unwrap();
        let sealed = seal(&id, b"top secret directive", b"aad").unwrap();
        let mut evil = sealed.clone();
        evil.ciphertext[0] ^= 0x01; // flip one bit
        assert!(matches!(
            unseal(&id, &evil, b"aad"),
            Err(AeadError::Decrypt)
        ));
    }

    #[test]
    fn wrong_aad_fails_closed() {
        let id = Identity::from_phrase(PHRASE).unwrap();
        let sealed = seal(&id, b"top secret directive", b"table:d-1").unwrap();
        // Relay routing header was swapped — reject.
        assert!(matches!(
            unseal(&id, &sealed, b"table:d-2"),
            Err(AeadError::Decrypt)
        ));
    }

    #[test]
    fn wrong_key_fails_closed() {
        let a = Identity::from_phrase(PHRASE).unwrap();
        let b = Identity::generate().unwrap();
        let sealed = seal(&a, b"payload", b"aad").unwrap();
        assert!(matches!(
            unseal(&b, &sealed, b"aad"),
            Err(AeadError::Decrypt)
        ));
    }

    #[test]
    fn wire_format_roundtrip() {
        let id = Identity::from_phrase(PHRASE).unwrap();
        let sealed = seal(&id, b"payload bytes", b"aad").unwrap();
        let wire = sealed.to_bytes();
        let back = Sealed::from_bytes(&wire).unwrap();
        let opened = unseal(&id, &back, b"aad").unwrap();
        assert_eq!(opened.as_slice(), b"payload bytes");
        // Truncated-by-5 wire blob still parses (nonce+tag intact) but
        // Poly1305 authentication must fail on the missing bytes.
        let mut tampered_wire = wire.clone();
        tampered_wire.truncate(wire.len() - 5);
        let parsed = Sealed::from_bytes(&tampered_wire).unwrap();
        assert!(matches!(
            unseal(&id, &parsed, b"aad"),
            Err(AeadError::Decrypt)
        ));
        // Severely truncated wire blobs (< nonce + tag floor) are rejected
        // outright by the parser.
        let truncated = &wire[..20];
        assert!(matches!(
            Sealed::from_bytes(truncated),
            Err(AeadError::Decrypt)
        ));
    }
}
