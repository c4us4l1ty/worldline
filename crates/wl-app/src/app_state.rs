//! Shared shell state: DB handle, identity (unlocked at runtime),
//! device id, relay session.

use std::path::Path;
use std::sync::Mutex;

use tauri::Manager;
use wl_core::crypto::identity::Identity;
use wl_core::store::repo::Repos;

use crate::vault::Vault;
use crate::ShellError;

pub struct RelaySession {
    pub base: String,
    pub account_id: String,
    pub token: String,
}

impl RelaySession {
    pub fn token_for(&self, base: &str, account_id: &str) -> Option<&str> {
        (self.base == base && self.account_id == account_id).then_some(self.token.as_str())
    }
}

pub struct AppState {
    pub repos: Repos,
    pub identity: Mutex<Option<Identity>>,
    pub vault: Vault,
    pub relay_token: Mutex<Option<RelaySession>>,
    pub relay_url: Mutex<Option<String>>,
}

impl AppState {
    pub fn new(app: &tauri::AppHandle) -> Result<Self, ShellError> {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|e| ShellError::Io(e.to_string()))?;
        std::fs::create_dir_all(&dir).map_err(|e| ShellError::Io(e.to_string()))?;
        let db = dir.join("worldline.sqlite");
        let conn = wl_core::store::open(&db)?;
        // Device id: low 2 bytes of a random UUID (non-secret),
        // persisted so CRDT tie-breaks stay stable across restarts.
        let repos = Repos::new(conn, device_id_for(&dir)?);
        // Pre-load the relay URL from persisted settings so sync works
        // immediately at boot without waiting for a settings save.
        // Tolerant: a fresh DB yields defaults (None) here.
        let relay_url = repos.settings().ok().and_then(|s| s.relay_url);
        // Vault open is fatal: without it neither identity unlock nor
        // BYOK calls can work, and silent fallback would lose keys.
        let vault = Vault::open(&dir).map_err(|e| ShellError::Vault(e.to_string()))?;
        Ok(Self {
            repos,
            identity: Mutex::new(None),
            vault,
            relay_token: Mutex::new(None),
            relay_url: Mutex::new(relay_url),
        })
    }

    /// Unlocked identity or error.
    pub fn with_identity<T>(
        &self,
        f: impl FnOnce(&Identity) -> Result<T, ShellError>,
    ) -> Result<T, ShellError> {
        let guard = self.identity.lock().expect("identity mutex");
        match guard.as_ref() {
            Some(id) => f(id),
            None => Err(ShellError::Locked),
        }
    }

    /// Unlocked identity when available, `None` when the vault is locked.
    /// Read paths and vault-independent writes use this; only sync and
    /// auth require the strict variant above.
    pub fn with_identity_opt<T>(
        &self,
        f: impl FnOnce(Option<&Identity>) -> Result<T, ShellError>,
    ) -> Result<T, ShellError> {
        let guard = self.identity.lock().expect("identity mutex");
        f(guard.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_session_is_scoped_to_endpoint_and_account() {
        let session = RelaySession {
            base: "http://127.0.0.1:8080".into(),
            account_id: "account-a".into(),
            token: "test-session".into(),
        };
        assert_eq!(
            session.token_for("http://127.0.0.1:8080", "account-a"),
            Some("test-session")
        );
        assert_eq!(session.token_for("http://127.0.0.1:8081", "account-a"), None);
        assert_eq!(session.token_for("http://127.0.0.1:8080", "account-b"), None);
    }
}

/// Stable per-install device id (0 < id < u16::MAX, never 0).
fn device_id_for(dir: &Path) -> Result<u16, ShellError> {
    let marker = dir.join("device_id");
    if let Ok(txt) = std::fs::read_to_string(&marker) {
        if let Ok(v) = txt.trim().parse::<u16>() {
            if v > 0 {
                return Ok(v);
            }
        }
    }
    let id = (uuid::Uuid::new_v4().as_u128() & 0xFFFF) as u16;
    let id = if id == 0 { 1 } else { id };
    std::fs::write(&marker, id.to_string()).map_err(|e| ShellError::Io(e.to_string()))?;
    Ok(id)
}
