//! Hardware-backed secret vault (Items 2+3): the Stronghold snapshot
//! holding the BIP-39 mnemonic and BYOK provider API keys.
//!
//! Threat model: the snapshot file is ChaCha-encrypted with a 32-byte
//! device-local key stored at `0600` in the app data dir. Secrets never
//! touch SQLite, the DOM, or the relay. Migrating the vault key into
//! the OS keychain (Keychain/Keystore/Secret Service) is tracked future
//! work — the `Vault` API is already shaped for it (`open` is the only
//! place that resolves key material).
//!
//! Record layout inside the `worldline` Stronghold client store:
//! * `mnemonic` — the 12-word phrase (UTF-8).
//! * `apikey:{provider}` — one entry per BYOK provider id.

use std::path::{Path, PathBuf};
use std::time::Duration;

use zeroize::Zeroizing;

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("io: {0}")]
    Io(String),
    #[error("stronghold: {0}")]
    Stronghold(String),
    #[error("no mnemonic stored in vault")]
    NoMnemonic,
}

pub struct Vault {
    stronghold: tauri_plugin_stronghold::stronghold::Stronghold,
}

impl Vault {
    /// Opens (or creates) the vault in `dir`. The snapshot lives at
    /// `dir/vault.hold`; its encryption key at `dir/.vault-key` (`0600`).
    pub fn open(dir: &Path) -> Result<Self, VaultError> {
        // Snapshot encryption defaults to a heavyweight password-KDF work
        // factor (scrypt, seconds per commit). Our snapshot key is a
        // 32-byte CSPRNG secret — not a password — so stretching buys
        // nothing and would stall every save. Zero it process-wide
        // (upstream documents 0 as correct for strong keys).
        let _ = iota_stronghold::engine::snapshot::try_set_encrypt_work_factor(0);
        let key = vault_key(dir)?;
        let snapshot = dir.join("vault.hold");
        let stronghold =
            tauri_plugin_stronghold::stronghold::Stronghold::new(&snapshot, key.to_vec())
                .map_err(|e| VaultError::Stronghold(e.to_string()))?;
        Ok(Self { stronghold })
    }

    // -- mnemonic (Item 3) --------------------------------------------

    pub fn save_mnemonic(&self, phrase: &str) -> Result<(), VaultError> {
        self.put(b"mnemonic", phrase.as_bytes())?;
        self.commit()
    }

    pub fn get_mnemonic(&self) -> Result<Zeroizing<String>, VaultError> {
        let bytes = self.get(b"mnemonic")?.ok_or(VaultError::NoMnemonic)?;
        String::from_utf8(bytes)
            .map(Zeroizing::new)
            .map_err(|_| VaultError::Stronghold("stored mnemonic is not valid UTF-8".into()))
    }

    pub fn has_mnemonic(&self) -> bool {
        self.get(b"mnemonic").map(|o| o.is_some()).unwrap_or(false)
    }

    // -- BYOK provider keys (Item 2) -----------------------------------

    /// Stores (or replaces) the API key for a provider id such as
    /// `openai-compat`, `openrouter`, or `anthropic`.
    pub fn save_api_key(&self, provider: &str, key: &str) -> Result<(), VaultError> {
        let record = api_key_record(provider)?;
        self.put(&record, key.as_bytes())?;
        self.commit()
    }

    /// Returns the key without ever logging or cloning it beyond the
    /// zeroizing wrapper the caller must drop promptly.
    pub fn get_api_key(&self, provider: &str) -> Result<Option<Zeroizing<String>>, VaultError> {
        let record = api_key_record(provider)?;
        match self.get(&record)? {
            Some(bytes) => String::from_utf8(bytes)
                .map(|s| Some(Zeroizing::new(s)))
                .map_err(|_| VaultError::Stronghold("stored key is not valid UTF-8".into())),
            None => Ok(None),
        }
    }

    pub fn has_api_key(&self, provider: &str) -> Result<bool, VaultError> {
        let record = api_key_record(provider)?;
        Ok(self.get(&record)?.is_some())
    }

    pub fn delete_api_key(&self, provider: &str) -> Result<bool, VaultError> {
        let record = api_key_record(provider)?;
        let existed = self.del(&record)?;
        if existed {
            self.commit()?;
        }
        Ok(existed)
    }

    // -- store primitives -----------------------------------------------

    fn client(
        &self,
    ) -> Result<iota_stronghold::Client, tauri_plugin_stronghold::stronghold::Error> {
        const CLIENT: &[u8] = b"worldline";
        match self.stronghold.get_client(CLIENT) {
            Ok(c) => Ok(c),
            // `load_snapshot` fills snapshot state, not the live clients
            // map: materialize the persisted client first, and only mint
            // a fresh one when the snapshot genuinely lacks it.
            Err(_) => match self.stronghold.load_client(CLIENT) {
                Ok(c) => Ok(c),
                Err(_) => self
                    .stronghold
                    .create_client(CLIENT)
                    .map_err(tauri_plugin_stronghold::stronghold::Error::from),
            },
        }
    }

    fn put(&self, key: &[u8], value: &[u8]) -> Result<(), VaultError> {
        let client = self
            .client()
            .map_err(|e| VaultError::Stronghold(e.to_string()))?;
        client
            .store()
            .insert(key.to_vec(), value.to_vec(), None::<Duration>)
            .map(|_| ())
            .map_err(|e| VaultError::Stronghold(e.to_string()))
    }

    fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, VaultError> {
        let client = self
            .client()
            .map_err(|e| VaultError::Stronghold(e.to_string()))?;
        client
            .store()
            .get(key)
            .map_err(|e| VaultError::Stronghold(e.to_string()))
    }

    fn del(&self, key: &[u8]) -> Result<bool, VaultError> {
        let client = self
            .client()
            .map_err(|e| VaultError::Stronghold(e.to_string()))?;
        client
            .store()
            .delete(key)
            .map(|o| o.is_some())
            .map_err(|e| VaultError::Stronghold(e.to_string()))
    }

    fn commit(&self) -> Result<(), VaultError> {
        self.stronghold
            .save()
            .map_err(|e| VaultError::Stronghold(e.to_string()))
    }
}

fn api_key_record(provider: &str) -> Result<Vec<u8>, VaultError> {
    let p = provider.trim();
    if p.is_empty() || p.len() > 64 || !p.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
        return Err(VaultError::Stronghold(format!(
            "invalid provider id {provider:?}"
        )));
    }
    Ok(format!("apikey:{p}").into_bytes())
}

/// Resolves the snapshot encryption key: existing `0600` file, or a
/// fresh 32-byte secret persisted `0600` on first run.
fn vault_key(dir: &Path) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    let path = dir.join(".vault-key");
    if let Ok(bytes) = std::fs::read(&path) {
        if bytes.len() == 32 {
            return Ok(Zeroizing::new(bytes));
        }
        return Err(VaultError::Stronghold(
            "vault key file has unexpected length".into(),
        ));
    }
    let mut key = vec![0u8; 32];
    getrandom::getrandom(&mut key).map_err(|e| VaultError::Stronghold(e.to_string()))?;
    write_private(&path, &key)?;
    Ok(Zeroizing::new(key))
}

#[cfg(unix)]
fn write_private(path: &PathBuf, bytes: &[u8]) -> Result<(), VaultError> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(bytes)
        })
        .map_err(|e| VaultError::Io(e.to_string()))
}

#[cfg(not(unix))]
fn write_private(path: &PathBuf, bytes: &[u8]) -> Result<(), VaultError> {
    // No Unix permission bits here; the app-data dir is already
    // user-private on these platforms. OS-keychain migration removes
    // this file entirely.
    std::fs::write(path, bytes).map_err(|e| VaultError::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wl-vault-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn api_key_roundtrip() {
        let dir = tmpdir();
        let v = Vault::open(&dir).unwrap();
        assert!(!v.has_api_key("openai-compat").unwrap());
        v.save_api_key("openai-compat", "sk-test-123").unwrap();
        assert!(v.has_api_key("openai-compat").unwrap());
        let got = v.get_api_key("openai-compat").unwrap().unwrap();
        assert_eq!(got.as_str(), "sk-test-123");
        // Provider namespaces are isolated.
        assert!(v.get_api_key("anthropic").unwrap().is_none());
        // Replace + delete.
        v.save_api_key("openai-compat", "sk-test-456").unwrap();
        assert_eq!(
            v.get_api_key("openai-compat").unwrap().unwrap().as_str(),
            "sk-test-456"
        );
        assert!(v.delete_api_key("openai-compat").unwrap());
        assert!(!v.has_api_key("openai-compat").unwrap());
        assert!(!v.delete_api_key("openai-compat").unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mnemonic_survives_reopen() {
        let dir = tmpdir();
        let phrase =
            "beacon orbit silence dynamic marble drift lattice kinetic harbor canyon velvet anchor";
        {
            let v = Vault::open(&dir).unwrap();
            assert!(v.get_mnemonic().is_err());
            v.save_mnemonic(phrase).unwrap();
            assert!(v.has_mnemonic());
        }
        // Drop + reopen: snapshot + key file persist.
        {
            if let Ok(md) = std::fs::metadata(dir.join("vault.hold")) {}
            let v = Vault::open(&dir).unwrap();
            assert!(v.has_mnemonic());
            assert_eq!(v.get_mnemonic().unwrap().as_str(), phrase);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_provider_rejected() {
        let dir = tmpdir();
        let v = Vault::open(&dir).unwrap();
        assert!(v.save_api_key("", "x").is_err());
        assert!(v.save_api_key("../escape", "x").is_err());
        assert!(v.save_api_key("has space", "x").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
