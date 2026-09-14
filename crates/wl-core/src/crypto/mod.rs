//! Cryptographic identity & payload encryption (PRD §3).
//!
//! Root of trust: 128-bit BIP-39 mnemonic. Everything is derived
//! locally via HKDF-SHA256 with fixed info labels; nothing leaves the
//! device unencrypted.

pub mod aead;
pub mod identity;

/// HKDF info label for the Ed25519 relay-signing subkey.
pub const HKDF_INFO_ED25519: &[u8] = b"wl/ed25519/v1";
/// HKDF info label for the ChaCha20-Poly1305 payload-encryption subkey.
pub const HKDF_INFO_CHACHA20: &[u8] = b"wl/chacha20/v1";
