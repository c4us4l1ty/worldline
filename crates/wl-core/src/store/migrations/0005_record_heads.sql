-- v5: per-record merge heads for total LWW arbitration (B-003/B-009).
--
-- The pull-apply path used to arbitrate on the row's own `hlc_timestamp`
-- (`WHERE hlc_timestamp < ?`), which has two holes: a missing row cannot
-- remember that it was deleted (older upserts resurrect it — B-003), and
-- two ops sharing one HLC arbitrate order-dependently (B-009). This table
-- mirrors the in-memory `TableState.last_op + tombstones` contract:
-- (hlc, device, operation_id) per (table, record), plus the delete bit.
CREATE TABLE IF NOT EXISTS record_heads (
    table_name TEXT NOT NULL,
    record_id TEXT NOT NULL,
    hlc_timestamp TEXT NOT NULL,
    device INTEGER NOT NULL,
    operation_id TEXT NOT NULL,
    tombstone INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (table_name, record_id)
);
