use rusqlite::Connection;

use super::StoreError;

/// Embedded migration runner (no C API dependency — keeps the crate
/// free of build-time tooling; migrations are plain SQL files).
const MIGRATIONS: &[&str] = &[
    include_str!("migrations/0001_init.sql"),
    include_str!("migrations/0002_sync_lww.sql"),
];

pub fn run(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at_epoch_ms INTEGER NOT NULL
        );",
    )?;
    for (idx, sql) in MIGRATIONS.iter().enumerate() {
        let version = (idx + 1) as i64;
        let applied: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version = ?1",
                [version],
                |r| r.get::<_, i64>(0),
            )
            .map(|c| c > 0)?;
        if !applied {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at_epoch_ms) VALUES (?1, ?2)",
                rusqlite::params![version, now_ms()],
            )?;
            tx.commit()?;
        }
    }
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before 1970")
        .as_millis() as i64
}
