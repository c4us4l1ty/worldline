-- Estimated Complexity (goal-level, 1–5) and explicit prerequisite edges
-- between directives. Both are replicated CRDT fields, so both need a
-- column, a JSON key in the payload, and an arm in the sync apply path.
--
-- `goals.complexity` is the user's own rating of how big the objective is.
-- It is the prior for the Beta-Bernoulli estimator in
-- `crate::engine::calibration`, and it is stored once per goal rather than
-- per directive on purpose: a goal-level rating is the thing the user can
-- actually answer before a plan exists, and inventing a per-directive
-- rating would be a number the model chose for itself.
--
-- `directives.depends_on` holds ONE prerequisite directive id, or NULL for
-- "nothing blocks this". It is checked in `Repos::runnable_directives`, one
-- hop, and never walked — see the note on `prerequisites_met` for why a
-- transitive chain walk over a column a peer can write is a hang waiting to
-- happen on a path the engine walks on every canvas load.
--
-- Both columns are additive and defaulted, so a mixed-version fleet is
-- safe: SQLite supplies the default for the omitted INSERT field, and an
-- old peer's payload simply lacks the key, which the apply path reads as
-- "no prerequisite" and "standard complexity" respectively. Same rule as
-- `app_settings.hotkey` / `always_on_top` (PRD deltas 169, 175).
--
-- NO `CHECK` on `complexity`, which is the decision worth stating. A
-- table-level CHECK looks like the safer choice and is not: the range is
-- validated in Rust at the write boundary (`domain::check_complexity`) and
-- clamped on read and on apply, and a CHECK here would make this build
-- REJECT a rating a future build widens the scale to — quarantining the op
-- and silently losing the field on exactly the peers that are merely
-- behind. The same reasoning already removed the pin-column problem
-- (delta 175).
ALTER TABLE directives ADD COLUMN depends_on TEXT;

ALTER TABLE goals ADD COLUMN complexity INTEGER NOT NULL DEFAULT 3;

CREATE INDEX idx_directives_depends_on ON directives(depends_on);
