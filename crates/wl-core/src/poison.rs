//! Poison-tolerant locking (SHELL-1, AUDIT-5).
//!
//! A `Mutex` is poisoned when a holder thread panics without unwinding
//! the guard. Every lock site in Worldline previously called
//! `lock().unwrap()` / `lock().expect(..)`, which turned one panicking
//! writer into a **process-wide fail-stop**: the app (or the relay, or
//! the HLC on every row mutation) could never take that lock again, so
//! the user's data dir became permanently unusable with no recovery path
//! short of deleting it.
//!
//! Poisoning is recoverable in every guarded field in this repo:
//!
//! * **Plain in-process state** — the HLC head, the relay's
//!   challenge/session maps, the shell's `Option<Identity>` /
//!   `Option<BearerToken>` — is plain data with no invariant a panic
//!   could have half-applied. `into_inner()` is exactly right.
//! * **The SQLite handle** is transactional, so a panicking writer has
//!   either committed or not. The one hazard is a transaction left open
//!   mid-flight, which would make the next reader see uncommitted rows;
//!   [`lock_conn`] rolls that back before handing the guard over.
//!
//! The alternative — propagating poison as an error — was rejected: a
//! single `Mutex<Connection>` has no way to manufacture a fresh
//! connection, so every call site would have to grow a restart path for
//! a condition that is strictly less bad than stranding the user.

use std::sync::{Mutex, MutexGuard};

use rusqlite::Connection;

/// Locks a mutex guarding plain in-process state, recovering from
/// poisoning rather than panicking.
pub trait LockRecover<T> {
    /// Acquire the lock, taking the inner value even if poisoned.
    fn lock_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> LockRecover<T> for Mutex<T> {
    fn lock_recover(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Locks the SQLite handle, recovering from poisoning and clearing a
/// half-open transaction left behind by a panicking writer.
pub fn lock_conn(conn: &Mutex<Connection>) -> MutexGuard<'_, Connection> {
    let guard = conn.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    // A writer that panicked between BEGIN and COMMIT leaves the handle
    // out of autocommit. Roll back so the next reader never observes
    // uncommitted rows, and so `Transaction::new` (which requires
    // autocommit) does not fail closed on a poisoned-but-usable handle.
    if !guard.is_autocommit() {
        let _ = guard.execute_batch("ROLLBACK");
    }
    guard
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn plain_lock_recovers_from_poisoning() {
        let m = Arc::new(Mutex::new(7u32));
        let clone = Arc::clone(&m);
        let t = std::thread::spawn(move || {
            let _held = clone.lock().unwrap();
            panic!("poison the mutex on purpose");
        });
        assert!(t.join().is_err(), "spawned thread should have panicked");
        assert!(m.is_poisoned(), "precondition: the mutex is poisoned");
        // The whole point: this used to panic.
        assert_eq!(*m.lock_recover(), 7);
    }

    #[test]
    fn lock_conn_recovers_and_rolls_back_open_transaction() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (x INTEGER);").unwrap();
        let m = Arc::new(Mutex::new(conn));

        // Leave a transaction open, then poison the mutex, exactly as a
        // panicking writer mid-transaction would.
        {
            let guard = m.lock().unwrap();
            guard
                .execute_batch("BEGIN; INSERT INTO t VALUES (1);")
                .unwrap();
        }
        {
            let clone = Arc::clone(&m);
            let t = std::thread::spawn(move || {
                let _held = clone.lock().unwrap();
                panic!("poison mid-transaction");
            });
            assert!(t.join().is_err());
        }
        assert!(m.is_poisoned(), "precondition: the mutex is poisoned");

        let guard = lock_conn(&m);
        assert!(
            guard.is_autocommit(),
            "lock_conn must roll back the half-open transaction"
        );
        let rows: i64 = guard
            .query_row("SELECT count(*) FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0, "the uncommitted insert must not be visible");
    }
}
