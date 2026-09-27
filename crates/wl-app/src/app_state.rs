//! Shared shell state: DB handle, identity (unlocked at runtime),
//! device id, relay session, provider model catalog cache.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::Manager;
use wl_core::ai::catalog::ModelInfo;
use wl_core::crypto::identity::Identity;
use wl_core::poison::LockRecover;
use wl_core::store::repo::Repos;

use crate::vault::Vault;
use crate::ShellError;
use crate::ShellResult;

/// How long a fetched model list is served from memory before a refetch.
///
/// Long enough that reopening Settings does not re-pull OpenRouter's
/// ~750 KB catalog every time, short enough that a model released
/// upstream appears on its own within the hour. `list_models` also takes
/// `force`, so waiting is never the only way to see a new model.
const CATALOG_TTL: Duration = Duration::from_secs(3600);

/// A cached catalog plus the moment it was fetched and whether the
/// fetch was itself truncated.
pub struct CatalogEntry {
    pub fetched_at: Instant,
    pub models: Vec<ModelInfo>,
    /// `Some(n)` when the provider's list was cut at
    /// [`wl_core::ai::catalog::MAX_MODELS`] and only `n` models were
    /// kept. Carried with the models so a cache hit reports the same
    /// thing the fetch did — otherwise a truncated catalog claims to be
    /// complete every time after the first.
    pub truncated_from: Option<usize>,
}

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
    /// The unlocked identity, held behind an `Arc` so a command that
    /// performs network I/O can keep using it after releasing the lock.
    /// See [`AppState::identity_handle`].
    pub identity: Mutex<Option<Arc<Identity>>>,
    pub vault: Vault,
    pub relay_token: Mutex<Option<RelaySession>>,
    /// Provider model lists, keyed by provider id.
    ///
    /// **Deliberately not in SQLite.** `app_settings` is a replicated
    /// CRDT table, so a catalog cached there would sync to every peer on
    /// the account and two devices would fight over one device-local
    /// cache. A catalog is a property of the provider at a point in
    /// time, not shared state, so it stays in process memory and is
    /// simply refetched.
    pub catalog: Mutex<HashMap<String, CatalogEntry>>,
}

impl AppState {
    pub fn new(app: &tauri::AppHandle) -> Result<Self, ShellError> {
        let dir = app
            .path()
            .app_data_dir()
            .map_err(|e| ShellError::Io(e.to_string()))?;
        // The vault opens FIRST because it is what puts the data
        // directory at `0700` (`vault::private_directory`). Opening the
        // database first created it `0644` inside a `0755` directory,
        // and that state is permanent whenever the vault then fails to
        // open — a populated, world-readable store left behind by an
        // app that refuses to start. Vault open is fatal by design:
        // without it neither identity unlock nor BYOK calls can work,
        // and silent fallback would lose keys.
        let vault = Vault::open(&dir).map_err(|e| ShellError::Vault(e.to_string()))?;
        let db = dir.join("worldline.sqlite");
        let conn = wl_core::store::open(&db)?;
        restrict_db_permissions(&db)?;
        // Device id: low 2 bytes of a random UUID (non-secret),
        // persisted so CRDT tie-breaks stay stable across restarts.
        let repos = Repos::new(conn, device_id_for(&dir)?);
        Ok(Self {
            repos,
            identity: Mutex::new(None),
            vault,
            relay_token: Mutex::new(None),
            catalog: Mutex::new(HashMap::new()),
        })
    }

    /// Cached catalog for a provider, if still inside the TTL, with the
    /// truncation flag the fetch reported.
    pub fn catalog_fresh(&self, provider: &str) -> Option<(Vec<ModelInfo>, Option<usize>)> {
        let guard = self.catalog.lock_recover();
        let entry = guard.get(provider)?;
        (entry.fetched_at.elapsed() < CATALOG_TTL)
            .then(|| (entry.models.clone(), entry.truncated_from))
    }

    /// One model from the fresh cache, used to decide whether a request
    /// may send `response_format` (see
    /// [`wl_core::ai::catalog::json_mode_for`]). A miss is `None`, which
    /// keeps the historical behaviour rather than guessing.
    pub fn cached_model(&self, provider: &str, model: &str) -> Option<ModelInfo> {
        let guard = self.catalog.lock_recover();
        let entry = guard.get(provider)?;
        if entry.fetched_at.elapsed() >= CATALOG_TTL {
            return None;
        }
        entry.models.iter().find(|m| m.id == model).cloned()
    }

    pub fn store_catalog(
        &self,
        provider: &str,
        models: Vec<ModelInfo>,
        truncated_from: Option<usize>,
    ) {
        let mut guard = self.catalog.lock_recover();
        guard.insert(
            provider.to_string(),
            CatalogEntry {
                fetched_at: Instant::now(),
                models,
                truncated_from,
            },
        );
    }

    /// The relay base to dial right now, read from the replicated
    /// settings row.
    ///
    /// This used to be a boot-time snapshot kept in its own `Mutex`,
    /// rewritten only by `settings_save`. `app_settings` is a
    /// replicated CRDT table, so a peer changing the relay URL landed
    /// in the row without touching that snapshot and the shell kept
    /// dialling — and caching a bearer token for — the old relay. There
    /// is nothing to keep in sync now that the row is the only copy.
    pub fn relay_base(&self) -> ShellResult<String> {
        let configured = self
            .repos
            .settings()?
            .relay_url
            .ok_or(ShellError::NoRelay)?;
        wl_core::net::validate_relay_url(&configured).map_err(ShellError::Invalid)
    }

    /// Unlocked identity or error.
    pub fn with_identity<T>(
        &self,
        f: impl FnOnce(&Identity) -> Result<T, ShellError>,
    ) -> Result<T, ShellError> {
        let guard = self.identity.lock_recover();
        match guard.as_deref() {
            Some(id) => f(id),
            None => Err(ShellError::Locked),
        }
    }

    /// Unlocked identity when available, `None` when the vault is locked.
    /// Read paths and vault-independent writes use this; only sync and
    /// auth require the strict variant above.
    ///
    /// The guard is held only for the closure. Anything that talks to
    /// the network must use [`AppState::identity_handle`] instead.
    pub fn with_identity_opt<T>(
        &self,
        f: impl FnOnce(Option<&Identity>) -> Result<T, ShellError>,
    ) -> Result<T, ShellError> {
        let guard = self.identity.lock_recover();
        f(guard.as_deref())
    }

    /// A shareable handle to the unlocked identity, taken without
    /// holding the lock.
    ///
    /// `master_plan` and `sync_now` need the identity across a provider
    /// or relay request that can take the better part of two minutes.
    /// Holding the mutex for that window made every other
    /// identity-gated command — `identity_status`, `check_in`,
    /// `settings_save`, the canvas — block a tokio worker thread behind
    /// the slowest request in the app, and two concurrent waiters were
    /// enough to starve the runtime. The `Arc` costs one atomic
    /// increment and leaves the lock free for the whole request.
    pub fn identity_handle(&self) -> Option<Arc<Identity>> {
        self.identity.lock_recover().clone()
    }
}

/// Tightens the SQLite file to `0600`, the same envelope as the vault
/// snapshot and its key. The `0700` parent directory is the real
/// boundary; this stops the file from being readable through a bind
/// mount, a backup tool, or anything else that reaches past the
/// directory mode.
#[cfg(unix)]
fn restrict_db_permissions(path: &Path) -> Result<(), ShellError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| ShellError::Io(e.to_string()))
}

#[cfg(not(unix))]
fn restrict_db_permissions(_path: &Path) -> Result<(), ShellError> {
    Ok(())
}

/// Stable per-install device id (0 < id < u16::MAX, never 0).
fn device_id_for(dir: &Path) -> Result<u16, ShellError> {
    let marker = dir.join("device_id");
    match std::fs::read_to_string(&marker) {
        Ok(txt) => {
            return txt
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|id| *id > 0)
                .ok_or_else(|| ShellError::Io("invalid persisted device id".into()));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(ShellError::Io(e.to_string())),
    }
    let id = (uuid::Uuid::new_v4().as_u128() & 0xFFFF) as u16;
    let id = if id == 0 { 1 } else { id };
    std::fs::write(&marker, id.to_string()).map_err(|e| ShellError::Io(e.to_string()))?;
    Ok(id)
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
        assert_eq!(
            session.token_for("http://127.0.0.1:8081", "account-a"),
            None
        );
        assert_eq!(
            session.token_for("http://127.0.0.1:8080", "account-b"),
            None
        );
    }

    #[test]
    fn invalid_device_marker_is_not_replaced() {
        let dir = std::env::temp_dir().join(format!("wl-device-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let marker = dir.join("device_id");
        for value in [b"0".as_slice(), b"corrupt", b"65536", b"\xff"] {
            std::fs::write(&marker, value).unwrap();
            assert!(device_id_for(&dir).is_err());
            assert_eq!(std::fs::read(&marker).unwrap(), value);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
