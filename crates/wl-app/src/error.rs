//! Shell error type surfaced to the UI as JSON.

#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    #[error("identity locked: unlock the vault first")]
    Locked,
    #[error("invalid mnemonic phrase")]
    BadMnemonic,
    #[error("store: {0}")]
    Store(#[from] wl_core::store::StoreError),
    #[error("engine: {0}")]
    Engine(#[from] wl_core::engine::EngineError),
    #[error("sync: {0}")]
    Sync(#[from] wl_sync::sync::SyncError),
    #[error("ai: {0}")]
    Ai(#[from] wl_core::ai::DispatchError),
    /// A provider call that failed for a reason `DispatchError` does not
    /// model — an HTTP status, a catalog read, an unusable body. The
    /// inner string is already human-readable
    /// (`ai::catalog::provider_error_message`), which is the point: it
    /// reaches the user instead of collapsing to "OFFLINE".
    #[error("{0}")]
    Provider(String),
    #[error("io: {0}")]
    Io(String),
    #[error("invalid argument: {0}")]
    Invalid(String),
    #[error("no relay configured")]
    NoRelay,
    #[error("relay handshake failed: {0}")]
    Relay(String),
    #[error("vault: {0}")]
    Vault(String),
    #[error("no API key stored for provider {0}")]
    NoApiKey(String),
}

impl serde::Serialize for ShellError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

pub type ShellResult<T> = Result<T, ShellError>;
