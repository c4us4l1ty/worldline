-- v2: sync-correctness hardening.
--
-- 1) LWW guard for directive_phases: apply-side arbitration needs a
--    comparable hlc per phase row (the pull path previously
--    blind-upserted phase state, so stale ops overwrote phase progress).
-- 2) identity_config.verify_indices: the 3-word backup challenge is
--    verified POSITIONALLY; the positions must survive restarts.
-- 3) sync_cursor: the pull cursor is persisted after every cycle
--    (previously re-pulled the entire remote history from "" each time).

ALTER TABLE directive_phases ADD COLUMN hlc_timestamp TEXT NOT NULL DEFAULT '0';

ALTER TABLE identity_config ADD COLUMN verify_indices TEXT NOT NULL DEFAULT '[]';

CREATE TABLE sync_cursor (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    cursor TEXT NOT NULL
);
