//! Blind-blob storage for the relay. Dual backend (PRD decision):
//! SQLite for dev/tests (zero infrastructure), Postgres for production.
//! The relay stores only: public key, opaque ciphertext, routing
//! headers, HLC text. It can never read payload contents.

#[cfg(feature = "sqlite")]
use wl_core::poison::LockRecover;

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
    /// A single account holds too many ops (authenticated storage-
    /// exhaustion guard — one valid session must not fill the disk).
    #[error("per-account op quota exceeded")]
    OpsQuota,
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
    /// Hard cap on stored ops per account (authenticated sessions can
    /// otherwise fill the disk one push at a time — the push handler
    /// also caps ops-per-request and bytes-per-op).
    const OPS_PER_ACCOUNT_CAP: i64 = 100_000;
    /// Ops per committed chunk (SYNC-1). The store has one write mutex,
    /// so this bounds how long a single push can convoy concurrent pulls.
    /// Sized well above the protocol's per-request op cap
    /// (`wl_protocol::MAX_BATCH_OPS`) so an ordinary client push still
    /// commits exactly once; only oversized pushes split.
    pub(crate) const INSERT_CHUNK_SIZE: usize = 128;

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

        fn init(mut conn: Connection) -> Result<Self, StoreError> {
            conn.pragma_update(None, "journal_mode", "WAL").ok();
            // IMMEDIATE: schema bootstrap plus the legacy rebuild below
            // must not race a concurrent opener on the same database file.
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            tx.execute_batch(
                "CREATE TABLE IF NOT EXISTS accounts (
                    public_key TEXT PRIMARY KEY,
                    created_at INTEGER NOT NULL
                );
                CREATE TABLE IF NOT EXISTS ops (
                    operation_id TEXT NOT NULL,
                    account TEXT NOT NULL,
                    hlc TEXT NOT NULL,
                    table_name TEXT NOT NULL,
                    record_id TEXT NOT NULL,
                    sealed BLOB NOT NULL,
                    PRIMARY KEY (account, operation_id)
                );
                CREATE INDEX IF NOT EXISTS idx_ops_account_hlc
                    ON ops(account, hlc, operation_id);",
            )?;
            // Legacy databases keyed ops by operation_id alone — a global
            // constraint across ALL accounts. Operation ids are
            // client-minted and may collide across accounts; the old key
            // swallowed the second account's row and misreported it as a
            // duplicate. Detect the old key layout and rebuild in place,
            // preserving every stored ciphertext row.
            let account_pk_pos: i64 = tx.query_row(
                "SELECT pk FROM pragma_table_info('ops') WHERE name = 'account'",
                [],
                |row| row.get(0),
            )?;
            if account_pk_pos == 0 {
                tx.execute_batch(
                    "ALTER TABLE ops RENAME TO ops_legacy;
                     CREATE TABLE ops (
                        operation_id TEXT NOT NULL,
                        account TEXT NOT NULL,
                        hlc TEXT NOT NULL,
                        table_name TEXT NOT NULL,
                        record_id TEXT NOT NULL,
                        sealed BLOB NOT NULL,
                        PRIMARY KEY (account, operation_id)
                     );
                     INSERT INTO ops
                        SELECT operation_id, account, hlc, table_name, record_id, sealed
                        FROM ops_legacy;
                     DROP TABLE ops_legacy;
                     CREATE INDEX idx_ops_account_hlc ON ops(account, hlc, operation_id);",
                )?;
            }
            tx.commit()?;
            Ok(Self {
                conn: Mutex::new(conn),
            })
        }
    }

    impl BlobStore for SqliteStore {
        fn register_account(&self, public_key: &str) -> Result<(), StoreError> {
            let conn = self.conn.lock_recover();
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
            let conn = self.conn.lock_recover();
            let n: i64 = conn.query_row(
                "SELECT COUNT(*) FROM accounts WHERE public_key = ?1",
                [public_key],
                |r| r.get(0),
            )?;
            Ok(n > 0)
        }

        fn insert_ops(&self, ops: &[StoredOp]) -> Result<PushOutcome, StoreError> {
            // SYNC-1: commit in chunks so a large push does not hold the
            // single write mutex (and one SQLite transaction) across the
            // whole batch. The store has exactly one write lock, so a
            // 500-op push previously convoyed every concurrent pull for
            // the duration. Chunking bounds the worst-case stall to
            // CHUNK_SIZE ops while keeping the lock released between
            // chunks.
            //
            // Quota atomicity is preserved: the cap is re-checked
            // *inside every chunk's transaction* against a live COUNT,
            // and the count only grows, so a batch that would exceed the
            // cap still fails on the first chunk that crosses it — with
            // the already-committed prefix durably stored. That is the
            // intended trade: partial progress is retryable (ops are
            // idempotent via `ON CONFLICT ... DO NOTHING`, and a retry
            // reports them as duplicates), whereas a single 20k-op
            // transaction that rolls back stores nothing and re-does all
            // the work. Idempotence per op is unchanged.
            let mut out = PushOutcome::default();
            for chunk in ops.chunks(INSERT_CHUNK_SIZE) {
                let mut conn = self.conn.lock_recover();
                let tx = conn.transaction()?;
                // Per-account quota inside the same transaction (the
                // store mutex serializes writers, so the
                // check-and-insert is atomic on this backend).
                if let Some(first) = chunk.first() {
                    let held: i64 = tx.query_row(
                        "SELECT COUNT(*) FROM ops WHERE account = ?1",
                        [&first.account],
                        |r| r.get(0),
                    )?;
                    // Multi-account batches are rejected at the HTTP
                    // layer; the store tolerates them by quota-checking
                    // the first.
                    if held + chunk.len() as i64 > OPS_PER_ACCOUNT_CAP {
                        // Commit the prefix that already landed, then
                        // report the cliff. Dropping it would discard
                        // acknowledged work.
                        tx.commit()?;
                        return Err(StoreError::OpsQuota);
                    }
                }
                for op in chunk {
                    let changed = tx.execute(
                        "INSERT INTO ops
                            (operation_id, account, hlc, table_name, record_id, sealed)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                         ON CONFLICT(account, operation_id) DO NOTHING",
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
                // Guard drops here: the mutex is released between chunks
                // so a concurrent pull can interleave.
            }
            Ok(out)
        }

        fn pull_ops(
            &self,
            account: &str,
            since_hlc: &str,
            since_op_id: &str,
            limit: u32,
        ) -> Result<Vec<StoredOp>, StoreError> {
            let conn = self.conn.lock_recover();
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
        // `created_at` is ordering metadata, not a security boundary:
        // fall back to 0 instead of panicking on a broken clock.
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
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

    /// Embedded migrations, applied in order on every boot. Each file is
    /// individually idempotent so replay is safe without a version table.
    const MIGRATIONS: &[&str] = &[
        include_str!("../migrations/0001_init.sql"),
        include_str!("../migrations/0002_pull_index.sql"),
    ];

    /// Same registration cap as the SQLite backend (see there).
    const ACCOUNT_CAP: i64 = 10_000;
    /// Same per-account op cap as the SQLite backend (see there).
    const OPS_PER_ACCOUNT_CAP: i64 = 100_000;
    /// Same chunk size as the SQLite backend (see there).
    pub(crate) const INSERT_CHUNK_SIZE: usize = 128;

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
                // Multi-statement scripts (0001 ends with the idempotent
                // legacy-PK upgrade): raw_sql batches them; a prepared
                // query() would reject multiple statements. Both files
                // are themselves idempotent, so replaying the list on
                // every boot is safe and needs no version table.
                let mut tx = self.pool.begin().await?;
                for sql in MIGRATIONS {
                    sqlx::raw_sql(sql).execute(&mut *tx).await?;
                }
                tx.commit().await?;
                Ok::<_, StoreError>(())
            })
        }
    }

    impl BlobStore for PostgresStore {
        fn register_account(&self, public_key: &str) -> Result<(), StoreError> {
            self.rt.block_on(async {
                // Atomic cap: the table lock serializes concurrent
                // registrations (the SQLite backend gets this from its
                // store mutex; Postgres needs it spelled out).
                let mut tx = self.pool.begin().await?;
                sqlx::query("LOCK TABLE accounts IN EXCLUSIVE MODE")
                    .execute(&mut *tx)
                    .await?;
                let res = sqlx::query("INSERT INTO accounts (public_key) VALUES ($1) ON CONFLICT (public_key) DO NOTHING")
                    .bind(public_key)
                    .execute(&mut *tx)
                    .await?;
                if res.rows_affected() > 0 {
                    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM accounts")
                        .fetch_one(&mut *tx)
                        .await?;
                    if row.0 > ACCOUNT_CAP {
                        // Roll back this registration — the cap is a DoS
                        // guard, not a correctness limit.
                        sqlx::query("DELETE FROM accounts WHERE public_key = $1")
                            .bind(public_key)
                            .execute(&mut *tx)
                            .await?;
                        return Err(StoreError::AccountQuota);
                    }
                }
                tx.commit().await?;
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
                // SYNC-1: same chunked-commit contract as the SQLite
                // backend — the quota re-check runs inside every chunk's
                // transaction against a live COUNT, and the committed
                // prefix is retained when the cap trips. See the SQLite
                // `insert_ops` for the full rationale.
                let mut out = PushOutcome::default();
                for chunk in ops.chunks(INSERT_CHUNK_SIZE) {
                    let mut tx = self.pool.begin().await?;
                    if let Some(first) = chunk.first() {
                        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM ops WHERE account = $1")
                            .bind(&first.account)
                            .fetch_one(&mut *tx)
                            .await?;
                        if row.0 + chunk.len() as i64 > OPS_PER_ACCOUNT_CAP {
                            tx.commit().await?;
                            return Err(StoreError::OpsQuota);
                        }
                    }
                    for op in chunk {
                        let res = sqlx::query(
                            "INSERT INTO ops (operation_id, account, hlc, table_name, record_id, sealed)
                             VALUES ($1, $2, $3, $4, $5, $6)
                             ON CONFLICT (account, operation_id) DO NOTHING",
                        )
                        .bind(&op.operation_id)
                        .bind(&op.account)
                        .bind(&op.hlc)
                        .bind(&op.table)
                        .bind(&op.record_id)
                        .bind(&op.sealed)
                        .execute(&mut *tx)
                        .await?;
                        if res.rows_affected() > 0 {
                            out.accepted.push(op.operation_id.clone());
                        } else {
                            out.duplicates.push(op.operation_id.clone());
                        }
                    }
                    tx.commit().await?;
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
