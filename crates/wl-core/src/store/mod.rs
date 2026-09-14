//! SQLite persistence: WAL mode, versioned migrations, repositories,
//! and the CRDT outbox (PRD §7).
//!
//! Uses rusqlite (synchronous) — local-first single-writer access is
//! the dominant pattern; async wrappers live in the Tauri shell layer.

pub mod migrations;
pub mod repo;

#[cfg(test)]
mod tests;

use std::path::Path;

use rusqlite::Connection;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("migration error: {0}")]
    Migration(String),
    #[error("row not found: {0}")]
    NotFound(String),
    #[error("invalid data: {0}")]
    Invalid(String),
}

/// Opens (or creates) the Worldline database with WAL mode and
/// sensible durability pragmas, then runs pending migrations.
pub fn open(path: &Path) -> Result<Connection, StoreError> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrations::run(&conn)?;
    Ok(conn)
}

/// In-memory database (tests / ephemeral sessions).
pub fn open_in_memory() -> Result<Connection, StoreError> {
    let conn = Connection::open_in_memory()?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    migrations::run(&conn)?;
    Ok(conn)
}
