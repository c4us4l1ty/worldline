-- Relay Postgres schema (production backend).
CREATE TABLE IF NOT EXISTS accounts (
    public_key TEXT PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS ops (
    operation_id TEXT NOT NULL,
    account TEXT NOT NULL REFERENCES accounts(public_key),
    hlc TEXT NOT NULL,
    table_name TEXT NOT NULL,
    record_id TEXT NOT NULL,
    sealed BYTEA NOT NULL,
    PRIMARY KEY (account, operation_id)
);
CREATE INDEX IF NOT EXISTS idx_ops_account_hlc ON ops(account, hlc);

-- Deduplication is scoped per account: operation ids are client-minted
-- and may legitimately collide across accounts; the original global
-- primary key let one account's row swallow another's. Upgrade in
-- place, idempotently; no ciphertext rows change.
DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_index i
        JOIN pg_attribute a ON a.attrelid = i.indrelid
             AND a.attnum = ANY (i.indkey::smallint[])
        WHERE i.indrelid = 'ops'::regclass
          AND i.indisprimary
          AND a.attname = 'operation_id'
    ) AND NOT EXISTS (
        SELECT 1 FROM pg_index i
        JOIN pg_attribute a ON a.attrelid = i.indrelid
             AND a.attnum = ANY (i.indkey::smallint[])
        WHERE i.indrelid = 'ops'::regclass
          AND i.indisprimary
          AND a.attname = 'account'
    ) THEN
        ALTER TABLE ops DROP CONSTRAINT ops_pkey;
        ALTER TABLE ops ADD PRIMARY KEY (account, operation_id);
    END IF;
END
$$;
