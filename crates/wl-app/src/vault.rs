//! Local secret vault (Items 2+3): the Stronghold snapshot holding the
//! BIP-39 mnemonic and BYOK provider API keys.
//!
//! Threat model: the snapshot file is XChaCha20-Poly1305-encrypted with a
//! 32-byte device-local key stored at `0600` in the app data dir, and the
//! snapshot KDF work factor is deliberately 0 (the key is a CSPRNG
//! secret, not a password — see PRD-DELTAS #20). This is a local
//! encrypted file, NOT an OS keychain and NOT hardware-backed. What is
//! guaranteed: secrets never reach SQLite and never reach the relay.
//! They do necessarily pass through the webview DOM, because the user
//! types them there, and the mnemonic is displayed once by design.
//! Migrating the vault key into the OS keychain
//! (Keychain/Keystore/Secret Service) is tracked future work (SHELL-6) —
//! the `Vault` API is already shaped for it (`open` is the only place
//! that resolves key material).
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
    snapshot: PathBuf,
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
        private_directory(dir)?;
        let snapshot = dir.join("vault.hold");
        match std::fs::symlink_metadata(&snapshot) {
            Ok(meta) if meta.is_file() => restrict_permissions(&snapshot)?,
            Ok(_) => return Err(VaultError::Io("snapshot is not a regular file".into())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(VaultError::Io(e.to_string())),
        }
        let key = vault_key(dir)?;
        let stronghold =
            tauri_plugin_stronghold::stronghold::Stronghold::new(&snapshot, key.to_vec())
                .map_err(|e| VaultError::Stronghold(e.to_string()))?;
        Ok(Self {
            stronghold,
            snapshot,
        })
    }

    // -- mnemonic (Item 3) --------------------------------------------

    pub fn save_mnemonic(&self, phrase: &str) -> Result<(), VaultError> {
        let previous = self.get(b"mnemonic")?.map(Zeroizing::new);
        self.put(b"mnemonic", phrase.as_bytes())?;
        if let Err(error) = self.commit() {
            // Failed persistence must not make a new identity visible to
            // unlock while SQLite and the durable snapshot still use the old one.
            match previous {
                Some(bytes) => self.put(b"mnemonic", &bytes)?,
                None => {
                    self.del(b"mnemonic")?;
                }
            }
            return Err(error);
        }
        Ok(())
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
    /// `openrouter`, `google`, `qwen`, or `bytez.com`.
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
            .map_err(|e| VaultError::Stronghold(e.to_string()))?;
        restrict_permissions(&self.snapshot)
    }
}

fn api_key_record(provider: &str) -> Result<Vec<u8>, VaultError> {
    let p = provider.trim();
    // Dots are legal: the approved provider set contains `bytez.com` (B-001).
    if p.is_empty()
        || p.len() > 64
        || !p
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
    {
        return Err(VaultError::Stronghold(format!(
            "invalid provider id {provider:?}"
        )));
    }
    Ok(format!("apikey:{p}").into_bytes())
}

/// Resolves the snapshot encryption key: existing `0600` file, or a
/// fresh 32-byte secret persisted `0600` on first run. A pre-existing
/// key file with lax permissions is tightened to `0600` (same threat
/// envelope as the SQLite DB itself).
fn vault_key(dir: &Path) -> Result<Zeroizing<Vec<u8>>, VaultError> {
    let path = dir.join(".vault-key");
    match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => {
            restrict_permissions(&path)?;
            let bytes =
                Zeroizing::new(std::fs::read(&path).map_err(|e| VaultError::Io(e.to_string()))?);
            if bytes.len() != 32 {
                return Err(VaultError::Stronghold(
                    "vault key file has unexpected length".into(),
                ));
            }
            return Ok(bytes);
        }
        Ok(_) => return Err(VaultError::Io("vault key is not a regular file".into())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(VaultError::Io(e.to_string())),
    }
    match std::fs::symlink_metadata(dir.join("vault.hold")) {
        Ok(_) => return Err(VaultError::Io("existing snapshot has no vault key".into())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(VaultError::Io(e.to_string())),
    }
    let mut key = Zeroizing::new(vec![0u8; 32]);
    getrandom::getrandom(&mut key).map_err(|e| VaultError::Stronghold(e.to_string()))?;
    write_private(&path, &key)?;
    Ok(key)
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<(), VaultError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| VaultError::Io(e.to_string()))
}

#[cfg(unix)]
fn private_directory(path: &Path) -> Result<(), VaultError> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(VaultError::Io(e.to_string())),
    }
    let meta = std::fs::symlink_metadata(path).map_err(|e| VaultError::Io(e.to_string()))?;
    if !meta.is_dir() {
        return Err(VaultError::Io("vault parent is not a directory".into()));
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| VaultError::Io(e.to_string()))
}

#[cfg(not(unix))]
fn private_directory(path: &Path) -> Result<(), VaultError> {
    std::fs::create_dir_all(path).map_err(|e| VaultError::Io(e.to_string()))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<(), VaultError> {
    Ok(())
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
        assert!(!v.has_api_key("openrouter").unwrap());
        v.save_api_key("openrouter", "sk-test-123").unwrap();
        assert!(v.has_api_key("openrouter").unwrap());
        let got = v.get_api_key("openrouter").unwrap().unwrap();
        assert_eq!(got.as_str(), "sk-test-123");
        // Provider namespaces are isolated.
        assert!(v.get_api_key("google").unwrap().is_none());
        // Replace + delete.
        v.save_api_key("openrouter", "sk-test-456").unwrap();
        assert_eq!(
            v.get_api_key("openrouter").unwrap().unwrap().as_str(),
            "sk-test-456"
        );
        assert!(v.delete_api_key("openrouter").unwrap());
        assert!(!v.has_api_key("openrouter").unwrap());
        assert!(!v.delete_api_key("openrouter").unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mnemonic_survives_reopen() {
        let dir = tmpdir();
        let phrase =
            "beacon orbit silence dynamic marble drift lattice kinetic harbor canyon velvet anchor";
        {
            assert!(!dir.join("vault.hold").exists());
            let v = Vault::open(&dir).unwrap();
            assert!(!dir.join("vault.hold").exists());
            assert!(matches!(v.get_mnemonic(), Err(VaultError::NoMnemonic)));
            v.save_mnemonic(phrase).unwrap();
            assert!(std::fs::metadata(dir.join("vault.hold")).unwrap().len() > 0);
            assert!(v.has_mnemonic());
        }
        // Drop + reopen: snapshot + key file persist.
        {
            let v = Vault::open(&dir).unwrap();
            assert!(v.has_mnemonic());
            assert_eq!(v.get_mnemonic().unwrap().as_str(), phrase);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_snapshot_is_retained() {
        for contents in [b"".as_slice(), b"not a stronghold snapshot".as_slice()] {
            let dir = tmpdir();
            let key = vault_key(&dir).unwrap();
            let snapshot = dir.join("vault.hold");
            std::fs::write(&snapshot, contents).unwrap();
            for _ in 0..2 {
                assert!(matches!(Vault::open(&dir), Err(VaultError::Stronghold(_))));
                assert_eq!(std::fs::read(&snapshot).unwrap(), contents);
                assert_eq!(std::fs::read(dir.join(".vault-key")).unwrap(), *key);
            }
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn snapshot_permissions_remain_private() {
        use std::os::unix::fs::PermissionsExt;

        fn mode(path: &Path) -> u32 {
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777
        }

        for existing_parent in [false, true] {
            let root = tmpdir();
            let dir = root.join("vault");
            if existing_parent {
                std::fs::create_dir(&dir).unwrap();
                std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            {
                let v = Vault::open(&dir).unwrap();
                assert_eq!(mode(&dir), 0o700);
                assert_eq!(mode(&dir.join(".vault-key")), 0o600);
                assert!(!dir.join("vault.hold").exists());
                for value in ["first", "replacement"] {
                    v.save_api_key("bytez.com", value).unwrap();
                    assert_eq!(mode(&dir), 0o700);
                    assert_eq!(mode(&dir.join("vault.hold")), 0o600);
                }
            }
            std::fs::set_permissions(
                dir.join("vault.hold"),
                std::fs::Permissions::from_mode(0o644),
            )
            .unwrap();
            {
                let v = Vault::open(&dir).unwrap();
                assert_eq!(mode(&dir.join("vault.hold")), 0o600);
                assert_eq!(
                    v.get_api_key("bytez.com").unwrap().unwrap().as_str(),
                    "replacement"
                );
                assert!(v.delete_api_key("bytez.com").unwrap());
                assert_eq!(mode(&dir.join("vault.hold")), 0o600);
            }
            std::fs::remove_dir_all(&root).unwrap();
        }
    }

    #[test]
    fn missing_key_does_not_replace_existing_snapshot() {
        let dir = tmpdir();
        {
            let vault = Vault::open(&dir).unwrap();
            vault.save_api_key("qwen", "test-key").unwrap();
        }
        let snapshot = std::fs::read(dir.join("vault.hold")).unwrap();
        std::fs::remove_file(dir.join(".vault-key")).unwrap();
        assert!(Vault::open(&dir).is_err());
        assert!(!dir.join(".vault-key").exists());
        assert_eq!(std::fs::read(dir.join("vault.hold")).unwrap(), snapshot);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn malformed_key_is_retained() {
        let dir = tmpdir();
        let path = dir.join(".vault-key");
        std::fs::write(&path, b"incomplete-key").unwrap();
        assert!(vault_key(&dir).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"incomplete-key");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn existing_key_permissions_are_repaired() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tmpdir();
        let key = vault_key(&dir).unwrap();
        let path = dir.join(".vault-key");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(*vault_key(&dir).unwrap(), *key);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_dir_all(dir).unwrap();
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
