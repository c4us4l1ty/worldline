-- 0006: `identity_config` is a singleton, not a set.
--
-- Why (CORE-4): the original key was `public_key TEXT PRIMARY KEY` with
-- `ON CONFLICT(public_key) DO UPDATE`. That makes every row a distinct
-- identity, so restoring a different phrase — or regenerating an
-- identity after a wipe — appended a SECOND row instead of replacing
-- the first. `identity()` then read `LIMIT 1`, which is an arbitrary
-- pick: the app could report the wrong public key, and
-- `set_mnemonic_verified` (a bare `UPDATE` with no `WHERE`) stamped the
-- verification flag onto *every* row at once.
--
-- Fix: an explicit `id = 1` primary key with a `CHECK`, so the schema
-- itself rejects a second row. Collapsing pre-existing duplicates takes
-- the earliest-inserted row, matching the old `LIMIT 1` (no `ORDER BY`
-- on a bare `SELECT` is rowid order in practice) so the migration does
-- not change which identity an upgrading install already trusts.
CREATE TABLE identity_config_singleton (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    public_key TEXT NOT NULL,
    bip39_mnemonic_verified BOOLEAN NOT NULL DEFAULT 0,
    verify_indices TEXT NOT NULL DEFAULT '[]',
    hlc_timestamp TEXT NOT NULL
);

INSERT INTO identity_config_singleton
    (id, public_key, bip39_mnemonic_verified, verify_indices, hlc_timestamp)
SELECT 1, public_key, bip39_mnemonic_verified, verify_indices, hlc_timestamp
FROM identity_config
ORDER BY rowid
LIMIT 1;

DROP TABLE identity_config;
ALTER TABLE identity_config_singleton RENAME TO identity_config;
