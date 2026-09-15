-- v3: LWW guard for app_settings (C1 remainder).
--
-- Every other synced table carries hlc_timestamp so the pull path can
-- arbitrate (WHERE hlc_timestamp < op.hlc); the singleton settings row
-- had none, so a stale settings op always overwrote newer local
-- settings by arrival order. Backfills '0' (older than any real tick).

ALTER TABLE app_settings ADD COLUMN hlc_timestamp TEXT NOT NULL DEFAULT '0';
