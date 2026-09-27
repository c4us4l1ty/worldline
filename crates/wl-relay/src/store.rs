//! Blind-blob storage for the relay. Dual backend (PRD decision):
//! SQLite for dev/tests (zero infrastructure), Postgres for production.
//! The relay stores only: public key, opaque ciphertext, routing
//! headers, HLC text. It can never read payload contents.

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

/// Lower bound on a stored op's serialized size, used to keep a pull
/// inside its byte budget while the rows are still being read.
///
/// Deliberately a *lower* bound rather than the exact serialized
/// length: this runs per row, on the path that decides whether to read
/// one more, so it must not serialise anything. The HTTP layer still
/// measures each op exactly before emitting it, and that measurement
/// is what the wire budget is enforced with — so the page the client
/// sees is unchanged, and this only has to keep memory bounded.
fn stored_bytes(op: &StoredOp) -> usize {
    op.operation_id.len()
        + op.account.len()
        + op.hlc.len()
        + op.table.len()
        + op.record_id.len()
        + op.sealed.len()
}

/// Storage backend trait.
pub trait BlobStore: Send + Sync {
    /// Ensures the account's challenge row exists (register on first
    /// auth challenge). Idempotent. When the relay is over its account
    /// cap this evicts the OLDEST accounts that hold no ops, so the
    /// storage bound holds without the cap ever becoming a permanent
    /// barrier to onboarding (DoS guard; see the SQLite backend).
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
    ///
    /// `max_bytes` is a budget on the RETURNED rows, enforced while
    /// they are read — not a hint the HTTP layer applies afterwards.
    /// A pull is the one route whose size scales with what an account
    /// has stored, and `limit` alone is not a size limit: 500 ops of
    /// 256 KiB is ~125 MiB per request, which one authenticated account
    /// can arrange and the concurrency limit then multiplies.
    ///
    /// Truncating on the byte budget is pagination-safe because the
    /// HTTP layer takes its cursor from the last row actually returned.
    fn pull_ops(
        &self,
        account: &str,
        since_hlc: &str,
        since_op_id: &str,
        limit: u32,
        max_bytes: usize,
    ) -> Result<Vec<StoredOp>, StoreError>;
}

// ---------------------------------------------------------------------------
// SQLite backend (dev / tests / small self-hosted deployments)
// ---------------------------------------------------------------------------

#[cfg(feature = "sqlite")]
pub mod sqlite_backend {
    use super::*;
    use rusqlite::Connection;
    use rusqlite::OptionalExtension;
    use wl_core::poison::lock_conn;
    // `lock_conn`, not `lock_recover`: this is the one
    // `Mutex<Connection>` in the workspace that was taking the plain
    // guard. `insert_ops` opens a transaction per chunk, so a panic
    // between `conn.transaction()` and `tx.commit()` leaves the
    // connection out of autocommit — and `lock_recover` hands that
    // state straight to the next caller, whose `conn.transaction()`
    // then fails permanently with "cannot start a transaction within a
    // transaction". That is unrecoverable without a restart, and it is
    // precisely the state `lock_conn` rolls back before handing the
    // guard over.
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

        /// Test support: how many accounts are resident. The cap is the
        /// relay's storage bound, and the property under test is that it
        /// HOLDS — which `register_account`'s return value cannot show
        /// once the cap evicts instead of refusing.
        #[doc(hidden)]
        pub fn account_row_count_for_test(&self) -> Result<i64, StoreError> {
            let conn = lock_conn(&self.conn);
            conn.query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))
                .map_err(StoreError::Sqlite)
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
            //
            // `optional()`: a database whose `ops` table predates the
            // `account` column returns NO ROW here, and that propagated
            // as `QueryReturnedNoRows` and aborted startup with a
            // misleading storage error. Such a table is exactly the one
            // that needs the rebuild below, so "cannot classify" has to
            // read as "needs rebuilding", not as a failure. `pk` is 1 for
            // a first primary-key column and 0 for a non-key column, so
            // the probe distinguishes "in the PK but not first" (the
            // legacy layout) from "already correct" — opposite rebuild
            // conditions.
            let account_pk_pos: Option<i64> = tx
                .query_row(
                    "SELECT pk FROM pragma_table_info('ops') WHERE name = 'account'",
                    [],
                    |row| row.get(0),
                )
                .optional()?;
            if account_pk_pos != Some(1) {
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
            let conn = lock_conn(&self.conn);
            let changed = conn.execute(
                "INSERT OR IGNORE INTO accounts (public_key, created_at) VALUES (?1, ?2)",
                rusqlite::params![public_key, now_ms()],
            )?;
            if changed == 0 {
                return Ok(()); // already registered
            }
            // The cap is a SLIDING WINDOW, not a one-shot ratchet.
            //
            // Registration is unauthenticated and the public key is
            // attacker-chosen, so 10 001 requests with 10 001 random keys
            // used to fill `accounts` permanently: the old code deleted
            // only the row it had just inserted, never reclaimed the
            // previous 10 000, and the table is a file that outlives the
            // process. Every legitimate new device then got 429 on
            // `/auth/challenge` and could never onboard, with no remedy
            // but a manual database edit.
            //
            // Evicting the OLDEST unreferenced rows instead bounds
            // resident accounts without deciding, permanently, who is
            // allowed to exist. An evicted account loses only its
            // relay-side row and re-registers on its next challenge; an
            // account that still holds ops is skipped, because `ops`
            // references `accounts` and taking its ciphertext with it
            // would be data loss.
            //
            // Eviction runs to a LOW-WATER MARK, not to the cap. Sweeping
            // one row at a time would re-run this ORDER BY over the whole
            // table on every single registration past the cap — turning
            // the flood guard into the flood. Dropping straight to 90 %
            // means the sweep runs once per 1 000 registrations instead
            // of once per registration.
            //
            // …and a sweep that removes NOTHING is not repeated. The
            // low-water mark only amortises while the sweep can actually
            // free enough rows to get back under the cap. Once most
            // accounts hold ops, the `LIMIT` applies to a candidate set
            // that is nearly empty, the DELETE frees nothing, and every
            // subsequent unauthenticated registration re-pays a full
            // correlated `NOT EXISTS` scan across the whole accounts
            // table — under the single store mutex on SQLite, and under a
            // relay-wide `pg_advisory_xact_lock` on Postgres. That is the
            // flood guard turned into the flood by a state it cannot fix.
            // Zero rows gone means the table is genuinely full, which is
            // a 429; an operator who needs more raises `ACCOUNT_CAP`.
            const LOW_WATER: i64 = ACCOUNT_CAP * 9 / 10;
            let held: i64 = conn.query_row("SELECT COUNT(*) FROM accounts", [], |r| r.get(0))?;
            if held > ACCOUNT_CAP {
                let swept = conn.execute(
                    "DELETE FROM accounts
                     WHERE public_key IN (
                         SELECT a.public_key FROM accounts a
                         WHERE NOT EXISTS (
                             SELECT 1 FROM ops o WHERE o.account = a.public_key
                         )
                         ORDER BY a.created_at ASC, a.public_key ASC
                         LIMIT max(0, (SELECT COUNT(*) FROM accounts) - ?1)
                     )",
                    rusqlite::params![LOW_WATER],
                )?;
                if swept == 0 {
                    return Err(StoreError::AccountQuota);
                }
            }
            Ok(())
        }

        fn account_exists(&self, public_key: &str) -> Result<bool, StoreError> {
            let conn = lock_conn(&self.conn);
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
                let mut conn = lock_conn(&self.conn);
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
            max_bytes: usize,
        ) -> Result<Vec<StoredOp>, StoreError> {
            let conn = lock_conn(&self.conn);
            // One statement, one bound-parameter list, one iterator.
            // Branching over two `query_map` calls leaves the loop below
            // with two iterator types to unify, and the byte budget has
            // to hold for both of them.
            let (sql, params): (&str, Vec<&dyn rusqlite::ToSql>) = if since_hlc.is_empty() {
                (
                    "SELECT operation_id, account, hlc, table_name, record_id, sealed
                     FROM ops WHERE account = ?1
                     ORDER BY hlc ASC, operation_id ASC LIMIT ?2",
                    vec![&account, &limit],
                )
            } else {
                (
                    "SELECT operation_id, account, hlc, table_name, record_id, sealed
                     FROM ops WHERE account = ?1
                       AND (hlc > ?2 OR (hlc = ?2 AND operation_id > ?3))
                     ORDER BY hlc ASC, operation_id ASC LIMIT ?4",
                    vec![&account, &since_hlc, &since_op_id, &limit],
                )
            };
            let mut stmt = conn.prepare(sql)?;
            let mut rows = stmt.query_map(params.as_slice(), op_row)?;
            // Streamed against a running byte total rather than
            // `collect()`-ed. The budget used to be applied by the HTTP
            // layer AFTER this returned, so one pull materialised
            // `min(limit, 500) × MAX_SEALED_BYTES` ≈ 125 MiB before a
            // single row had been decided not to be sent — times the
            // concurrency limit, for one authenticated account. Stopping
            // here bounds the peak at the budget plus one row.
            let mut out: Vec<StoredOp> = Vec::new();
            let mut used = 0usize;
            // `MappedRows::next` yields `Option<Result<T>>`, so the `?`
            // belongs on the item, not on the iteration step.
            while let Some(row) = rows.next() {
                let op = row?;
                used = used.saturating_add(stored_bytes(&op));
                if used > max_bytes {
                    break;
                }
                out.push(op);
            }
            Ok(out)
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
                // `pg_advisory_xact_lock` rather than
                // `LOCK TABLE accounts IN EXCLUSIVE MODE`.
                //
                // EXCLUSIVE conflicts with ROW EXCLUSIVE — the mode every
                // `insert_ops` takes — so taking it on an UNAUTHENTICATED
                // route serialized every challenge request against all op
                // writes relay-wide, and held the lock across a full
                // COUNT(*). An advisory lock on a fixed key gives the same
                // mutual exclusion between registrations, and conflicts
                // with nothing else.
                const REGISTRATION_LOCK: i64 = 0x_576c_5f72_6567;
                let mut tx = self.pool.begin().await?;
                sqlx::query("SELECT pg_advisory_xact_lock($1)")
                    .bind(REGISTRATION_LOCK)
                    .execute(&mut *tx)
                    .await?;
                let res = sqlx::query("INSERT INTO accounts (public_key) VALUES ($1) ON CONFLICT (public_key) DO NOTHING")
                    .bind(public_key)
                    .execute(&mut *tx)
                    .await?;
                if res.rows_affected() > 0 {
                    // Sliding window, mirroring the SQLite backend: over
                    // the cap, evict the oldest accounts that hold no ops
                    // rather than refusing. A one-shot ratchet filled the
                    // table permanently and blocked every new device
                    // forever. Low-water mark so the sweep runs once per
                    // batch of registrations, not once per registration.
                    //
                    // …and a sweep that removes NOTHING is not repeated
                    // (see the SQLite backend for why: the low-water
                    // mark only amortises while the sweep can free
                    // enough rows, and once most accounts hold ops this
                    // runs a full correlated scan under a relay-wide
                    // advisory lock on an unauthenticated route). Zero
                    // rows gone means genuinely full, which is a 429.
                    const LOW_WATER: i64 = ACCOUNT_CAP * 9 / 10;
                    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM accounts")
                        .fetch_one(&mut *tx)
                        .await?;
                    if row.0 > ACCOUNT_CAP {
                        let swept = sqlx::query(
                            "DELETE FROM accounts WHERE public_key IN (
                                 SELECT a.public_key FROM accounts a
                                 WHERE NOT EXISTS (
                                     SELECT 1 FROM ops o WHERE o.account = a.public_key
                                 )
                                 ORDER BY a.created_at ASC, a.public_key ASC
                                 LIMIT GREATEST(0, (SELECT COUNT(*) FROM accounts) - $1)
                             )",
                        )
                        .bind(LOW_WATER)
                        .execute(&mut *tx)
                        .await?;
                        if swept.rows_affected() == 0 {
                            return Err(StoreError::AccountQuota);
                        }
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
                        // The account row is (re)created here, inside the
                        // same transaction as the op — see the SQLite
                        // `insert_ops`. This backend carries a real FK
                        // (`ops.account REFERENCES accounts`), and the
                        // eviction sweep in `register_account` deletes any
                        // account holding no ops, so a device whose row
                        // had been swept could never push again: the
                        // insert failed, the handler mapped the unknown
                        // storage error to a 500, and a 500 triggers no
                        // re-handshake — so its outbox could not drain for
                        // the rest of the session. SQLite declares no FK
                        // and therefore diverged silently from this
                        // backend's failure mode for the identical
                        // request; the insert makes the two agree.
                        sqlx::query(
                            "INSERT INTO accounts (public_key) VALUES ($1)
                             ON CONFLICT (public_key) DO NOTHING",
                        )
                        .bind(&first.account)
                        .execute(&mut *tx)
                        .await?;
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
            max_bytes: usize,
        ) -> Result<Vec<StoredOp>, StoreError> {
            self.rt.block_on(async {
                // Sized against the byte budget, not just `limit`. The
                // budget used to be applied by the HTTP layer AFTER this
                // returned, so one pull materialised
                // `min(limit, 500) × MAX_SEALED_BYTES` ≈ 125 MiB before a
                // single row had been decided against sending. Two
                // statements rather than one: the length probe is
                // index-covered and moves no blob, and it bounds the real
                // fetch without streaming a cursor across the
                // `spawn_blocking` bridge the sync trait is called
                // through.
                let budget = max_bytes as i64;
                let wanted: i64 = {
                    let mut used: i64 = 0;
                    let mut n: i64 = 0;
                    let lengths: Vec<i64> = if since_hlc.is_empty() {
                        sqlx::query_scalar(
                            "SELECT octet_length(sealed)
                                 + length(operation_id) + length(account) + length(hlc)
                                 + length(table_name) + length(record_id)
                             FROM ops WHERE account = $1
                             ORDER BY hlc ASC, operation_id ASC LIMIT $2",
                        )
                        .bind(account)
                        .bind(limit as i64)
                        .fetch_all(&self.pool)
                        .await?
                    } else {
                        sqlx::query_scalar(
                            "SELECT octet_length(sealed)
                                 + length(operation_id) + length(account) + length(hlc)
                                 + length(table_name) + length(record_id)
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
                    for len in lengths {
                        if used.saturating_add(len) > budget {
                            break;
                        }
                        used = used.saturating_add(len);
                        n += 1;
                    }
                    n
                };
                if wanted == 0 {
                    return Ok(Vec::new());
                }
                let rows = if since_hlc.is_empty() {
                    sqlx::query_as::<_, (String, String, String, String, String, Vec<u8>)>(
                        "SELECT operation_id, account, hlc, table_name, record_id, sealed
                         FROM ops WHERE account = $1
                         ORDER BY hlc ASC, operation_id ASC LIMIT $2",
                    )
                    .bind(account)
                    .bind(wanted)
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
                    .bind(wanted)
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
