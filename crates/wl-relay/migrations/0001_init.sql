-- Relay Postgres schema (production backend).
CREATE TABLE IF NOT EXISTS accounts (
    public_key TEXT PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS ops (
    operation_id TEXT PRIMARY KEY,
    account TEXT NOT NULL REFERENCES accounts(public_key),
    hlc TEXT NOT NULL,
    table_name TEXT NOT NULL,
    record_id TEXT NOT NULL,
    sealed BYTEA NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_ops_account_hlc ON ops(account, hlc);
