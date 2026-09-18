use rusqlite::Connection;

use super::StoreError;

/// Embedded migration runner (no C API dependency — keeps the crate
/// free of build-time tooling; migrations are plain SQL files).
const MIGRATIONS: &[&str] = &[
    include_str!("migrations/0001_init.sql"),
    include_str!("migrations/0002_sync_lww.sql"),
    include_str!("migrations/0003_settings_hlc.sql"),
    include_str!("migrations/0004_hlc_zero_backfill.sql"),
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
    epoch_ms(std::time::SystemTime::now())
}

fn epoch_ms(now: std::time::SystemTime) -> i64 {
    now.duration_since(std::time::UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::epoch_ms;
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn migration_clock_before_epoch_is_zero() {
        assert_eq!(epoch_ms(UNIX_EPOCH - Duration::from_secs(1)), 0);
        assert_eq!(epoch_ms(UNIX_EPOCH + Duration::from_millis(1234)), 1234);
    }
}
