-- v4: repair the v2/v3 `'0'` HLC backfills.
--
-- `ALTER TABLE ... ADD COLUMN hlc_timestamp TEXT NOT NULL DEFAULT '0'`
-- stamped existing rows with the bare string '0', which is NOT a parseable
-- `pt.ctr.device` timestamp — any read of such a row fails mapping.
-- Rewrite to the canonical zero timestamp (older than every real tick,
-- so LWW arbitration is unaffected).

UPDATE directive_phases
SET hlc_timestamp = '00000000000000000000.00000.00000'
WHERE hlc_timestamp = '0';

UPDATE app_settings
SET hlc_timestamp = '00000000000000000000.00000.00000'
WHERE hlc_timestamp = '0';
