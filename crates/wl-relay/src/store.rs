//! Blind-blob storage for the relay. Dual backend (PRD decision):
//! SQLite for dev/tests (zero infrastructure), Postgres for production.
//! The relay stores only: public key, opaque ciphertext, routing
//! headers, HLC text. It can never read payload contents.

use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[cfg(feature = "sqlite")]
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[cfg(feature = "postgres")]
    #[error("postgres: {0}")]
    Postgres(#[from] sqlx::Error),
    /// Relay is at its account cap (unauthenticated registration DoS guard).
    #[error("account quota exceeded")]
    AccountQuota,
}

/// One stored relay op row.
#[derive(Debug, Clone)]
pub struct StoredOp {
    pub operation_id: String,
    pub account: String, // hex public key (opaque account id)
    pub hlc: String,
    pub table: String,
    pub record_id: String,
    pub sealed: Vec<u8>, // opaque ciphertext blob
}

/// Push outcome: which ops the relay newly stored vs. already held.
#[derive(Debug, Clone, Default)]
pub struct PushOutcome {
    /// Newly inserted operation ids.
    pub accepted: Vec<String>,
    /// Ids the relay already held (idempotent re-push) — callers use
    /// both lists to drain their outbox (a lost push response must not
    /// wedge an op in `pushed=0` forever).
    pub duplicates: Vec<String>,
}

/// Storage backend trait.
pub trait BlobStore: Send + Sync {
    /// Ensures the account's challenge row exists (register on first
    /// auth challenge). Idempotent. Fails with an account-quota error
    /// when the relay is at its account cap (DoS guard).
    fn register_account(&self, public_key: &str) -> Result<(), StoreError>;

    /// Checks the account exists.
    fn account_exists(&self, public_key: &str) -> Result<bool, StoreError>;

    /// Inserts ops; reports newly accepted ids separately from
    /// duplicates (both are durably stored — duplicates were already).
    fn insert_ops(&self, ops: &[StoredOp]) -> Result<PushOutcome, StoreError>;

    /// Pulls ops for an account strictly after the `(since_hlc,
    /// since_op_id)` cursor, ordered by `(hlc, operation_id)` ascending
    /// (deterministic), up to `limit`. Same-HLC ties advance via the
    /// operation id component.
    fn pull_ops(
        &self,
        account: &str,
        since_hlc: &str,
        since_op_id: &str,
        limit: u32,
    ) -> Result<Vec<StoredOp>, StoreError>;
}

// ---------------------------------------------------------------------------
// SQLite backend (dev / tests / small self-hosted deployments)
// ---------------------------------------------------------------------------

#[cfg(feature = "sqlite")]
pub mod sqlite_backend {
    use super::*;
    use rusqlite::Connection;
    use std::path::Path;
    use std::sync::Mutex;

    /// Hard cap on registered accounts. Registration is unauthenticated
    /// (public key = account id), so an attacker may mint rows for free;
    /// the cap bounds storage exhaustion. Self-hosted operators can
    /// raise it via env var.
    const ACCOUNT_CAP: i64 = 10_000;

    pub struct SqliteStore {
        conn: Mutex<Connection>,
    }

    impl SqliteStore {
        pub fn open(path: &Path) -> Result<Self, StoreError> {
            let conn = Connection::open(path)?;
            Self::init(conn)
        }

        pub fn open_in_memory() -> Result<Self, StoreError> {
            let conn = Connection::open_in_memory()?;
            Self::init(conn)
        }

        fn init(conn: Connection) -> Result<Self, StoreError> {
            conn.pragma_update(None, "journal_mode", "WAL").ok();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS accounts (
                    public_key TEXT PRIMARY KEY,
                    created_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS ops (
                    operation_id TEXT PRIMARY KEY,
                    account TEXT NOT NULL,
                    hlc TEXT NOT NULL,
                    table_name TEXT NOT NULL,
                    record_id TEXT NOT NULL,
                    sealed BLOB NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_ops_account_hlc
                    ON ops(account, hlc, operation_id);",
            )?;
            Ok(Self {
                conn: Mutex::new(conn),
            })
        }
    }

    impl BlobStore for SqliteStore {
        fn register_account(&self, public_key: &str) -> Result<(), StoreError> {
            let conn = self.conn.lock().expect("store mutex");
            let changed = conn.execute(
                "INSERT OR IGNORE INTO accounts (public_key, created_at) VALUES (?1, ?2)",
                rusqlite::params![public_key, now_ms()],
            )?;
            if changed == 0 {
                return Ok(()); // already registered
            }
            let count: i64 = conn.query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))?;
            if count > ACCOUNT_CAP {
                // Roll this registration back — the cap is a DoS guard,
                // not a correctness limit.
                conn.execute("DELETE FROM accounts WHERE public_key = ?1", [public_key])?;
                return Err(StoreError::AccountQuota);
            }
            Ok(())
        }

        fn account_exists(&self, public_key: &str) -> Result<bool, StoreError> {
            let conn = self.conn.lock().expect("store mutex");
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM accounts WHERE public_key = ?1",
                [public_key],
                |r| r.get(0),
            )?;
            Ok(n > 0)
        }

        fn insert_ops(&self, ops: &[StoredOp]) -> Result<PushOutcome, StoreError> {
            let mut conn = self.conn.lock().expect("store mutex");
            let tx = conn.transaction()?;
            let mut out = PushOutcome::default();
            for op in ops {
                let changed = tx.execute(
                    "INSERT OR IGNORE INTO ops
                        (operation_id, account, hlc, table_name, record_id, sealed)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        op.operation_id,
                        op.account,
                        op.hlc,
                        op.table,
                        op.record_id,
                        op.sealed
                    ],
                )?;
                if changed > 0 {
                    out.accepted.push(op.operation_id.clone());
                } else {
                    out.duplicates.push(op.operation_id.clone());
                }
            }
            tx.commit()?;
            Ok(out)
        }

        fn pull_ops(
            &self,
            account: &str,
            since_hlc: &str,
            since_op_id: &str,
            limit: u32,
        ) -> Result<Vec<StoredOp>, StoreError> {
            let conn = self.conn.lock().expect("store mutex");
            let mut stmt = if since_hlc.is_empty() {
                conn.prepare(
                    "SELECT operation_id, account, hlc, table_name, record_id, sealed
                     FROM ops WHERE account = ?1
                     ORDER BY hlc ASC, operation_id ASC LIMIT ?2",
                )?
            } else {
                conn.prepare(
                    "SELECT operation_id, account, hlc, table_name, record_id, sealed
                     FROM ops WHERE account = ?1
                       AND (hlc > ?2 OR (hlc = ?2 AND operation_id > ?3))
                     ORDER BY hlc ASC, operation_id ASC LIMIT ?4",
                )?
            };
            let rows = if since_hlc.is_empty() {
                stmt.query_map(rusqlite::params![account, limit], op_row)?
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                stmt.query_map(
                    rusqlite::params![account, since_hlc, since_op_id, limit],
                    op_row,
                )?
                .collect::<Result<Vec<_>, _>>()?
            };
            Ok(rows)
        }
    }

    fn op_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<StoredOp> {
        Ok(StoredOp {
            operation_id: r.get(0)?,
            account: r.get(1)?,
            hlc: r.get(2)?,
            table: r.get(3)?,
            record_id: r.get(4)?,
            sealed: r.get(5)?,
        })
    }

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before 1970")
            .as_millis() as i64
    }
}

// ---------------------------------------------------------------------------
// Postgres backend (production)
// ---------------------------------------------------------------------------

#[cfg(feature = "postgres")]
pub mod postgres_backend {
    use super::*;
    use sqlx::postgres::PgPoolOptions;
    use sqlx::PgPool;

    /// Same registration cap as the SQLite backend (see there).
    const ACCOUNT_CAP: i64 = 10_000;

    pub struct PostgresStore {
        pool: PgPool,
        /// Block on async sqlx from the sync trait methods.
        rt: tokio::runtime::Runtime,
    }

    impl PostgresStore {
        pub fn open(url: &str) -> Result<Self, StoreError> {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .map_err(|e| {
                    StoreError::Postgres(sqlx::Error::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        e.to_string(),
                    )))
                })?;
            let pool =
                rt.block_on(async { PgPoolOptions::new().max_connections(16).connect(url).await })?;
            let store = Self { pool, rt };
            store.migrate()?;
            Ok(store)
        }

        fn migrate(&self) -> Result<(), StoreError> {
            self.rt.block_on(async {
                sqlx::query(include_str!("../migrations/0001_init.sql"))
                    .execute(&self.pool)
                    .await?;
                Ok::<_, StoreError>(())
            })
        }
    }

    impl BlobStore for PostgresStore {
        fn register_account(&self, public_key: &str) -> Result<(), StoreError> {
            self.rt.block_on(async {
                sqlx::query("INSERT INTO accounts (public_key) VALUES ($1) ON CONFLICT (public_key) DO NOTHING")
                    .bind(public_key)
                    .execute(&self.pool)
                    .await?;
                let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM accounts")
                    .fetch_one(&self.pool)
                    .await?;
                if row.0 > ACCOUNT_CAP {
                    sqlx::query("DELETE FROM accounts WHERE public_key = $1")
                        .bind(public_key)
                        .execute(&self.pool)
                        .await?;
                    return Err(StoreError::AccountQuota);
                }
                Ok::<_, StoreError>(())
            })
        }

        fn account_exists(&self, public_key: &str) -> Result<bool, StoreError> {
            self.rt.block_on(async {
                let row: (i64,) =
                    sqlx::query_as("SELECT COUNT(*) FROM accounts WHERE public_key = $1")
                        .bind(public_key)
                        .fetch_one(&self.pool)
                        .await?;
                Ok::<_, StoreError>(row.0 > 0)
            })
        }

        fn insert_ops(&self, ops: &[StoredOp]) -> Result<PushOutcome, StoreError> {
            self.rt.block_on(async {
                let mut out = PushOutcome::default();
                for op in ops {
                    let res = sqlx::query(
                        "INSERT INTO ops (operation_id, account, hlc, table_name, record_id, sealed)
                         VALUES ($1, $2, $3, $4, $5, $6)
                         ON CONFLICT (operation_id) DO NOTHING",
                    )
                    .bind(&op.operation_id)
                    .bind(&op.account)
                    .bind(&op.hlc)
                    .bind(&op.table)
                    .bind(&op.record_id)
                    .bind(&op.sealed)
                    .execute(&self.pool)
                    .await?;
                    if res.rows_affected() > 0 {
                        out.accepted.push(op.operation_id.clone());
                    } else {
                        out.duplicates.push(op.operation_id.clone());
                    }
                }
                Ok::<_, StoreError>(out)
            })
        }

        fn pull_ops(
            &self,
            account: &str,
            since_hlc: &str,
            since_op_id: &str,
            limit: u32,
        ) -> Result<Vec<StoredOp>, StoreError> {
            self.rt.block_on(async {
                let rows = if since_hlc.is_empty() {
                    sqlx::query_as::<_, (String, String, String, String, String, Vec<u8>)>(
                        "SELECT operation_id, account, hlc, table_name, record_id, sealed
                         FROM ops WHERE account = $1
                         ORDER BY hlc ASC, operation_id ASC LIMIT $2",
                    )
                    .bind(account)
                    .bind(limit as i64)
                    .fetch_all(&self.pool)
                    .await?
                } else {
                    sqlx::query_as::<_, (String, String, String, String, String, Vec<u8>)>(
                        "SELECT operation_id, account, hlc, table_name, record_id, sealed
                         FROM ops WHERE account = $1
                           AND (hlc > $2 OR (hlc = $2 AND operation_id > $3))
                         ORDER BY hlc ASC, operation_id ASC LIMIT $4",
                    )
                    .bind(account)
                    .bind(since_hlc)
                    .bind(since_op_id)
                    .bind(limit as i64)
                    .fetch_all(&self.pool)
                    .await?
                };
                Ok::<_, StoreError>(
                    rows.into_iter()
                        .map(
                            |(operation_id, account, hlc, table, record_id, sealed)| StoredOp {
                                operation_id,
                                account,
                                hlc,
                                table,
                                record_id,
                                sealed,
                            },
                        )
                        .collect(),
                )
            })
        }
    }
}

/// Helper: parse a `PushOp` JSON body into `StoredOp`s.
#[allow(dead_code)]
pub fn ops_from_json(account: &str, body: &Value) -> Option<Vec<StoredOp>> {
    use base64::Engine;
    let ops = body["ops"].as_array()?;
    let mut out = Vec::with_capacity(ops.len());
    for op in ops {
        let sealed = base64::engine::general_purpose::STANDARD
            .decode(op["sealed_b64"].as_str()?)
            .ok()?;
        out.push(StoredOp {
            operation_id: op["operation_id"].as_str()?.to_string(),
            account: account.to_string(),
            hlc: op["hlc"].as_str()?.to_string(),
            table: op["table"].as_str()?.to_string(),
            record_id: op["record_id"].as_str()?.to_string(),
            sealed,
        });
    }
    Some(out)
}
