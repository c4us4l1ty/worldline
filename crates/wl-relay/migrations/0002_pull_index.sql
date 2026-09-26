-- 0002: align the Postgres pull index with the SQLite backend (SYNC-3).
--
-- Why: `pull_ops` paginates with
--     WHERE account = $1
--       AND (hlc > $2 OR (hlc = $2 AND operation_id > $3))
--     ORDER BY hlc ASC, operation_id ASC
-- so the index that serves it must be `(account, hlc, operation_id)`.
-- The SQLite backend has always had that tail (`store.rs`,
-- `ON ops(account, hlc, operation_id)`); Postgres had only
-- `(account, hlc)`. Postgres could therefore use the index for the
-- `account =` prefix but had to sort the whole account's tail by
-- `operation_id` on every pull — silently quadratic as an account
-- approaches the 100k op cap, and a pull-latency cliff the SQLite dev
-- path never showed.
--
-- `DROP INDEX IF EXISTS` then `CREATE INDEX IF NOT EXISTS` keeps this
-- idempotent and safe to re-run; it holds a brief exclusive lock on
-- `ops`, so operators should apply it during a quiet window.
DROP INDEX IF EXISTS idx_ops_account_hlc;

CREATE INDEX IF NOT EXISTS idx_ops_account_hlc_op
    ON ops(account, hlc, operation_id);
