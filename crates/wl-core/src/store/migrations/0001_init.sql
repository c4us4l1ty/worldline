-- Worldline client schema v1 (SQLite, WAL mode).
-- Extends PRD §7 with goals, directive_phases, check_ins, bailouts,
-- app_settings. HLC timestamps are stored as TEXT `pt.ctr.device`
-- everywhere (PRD delta: `created_at INTEGER` → `hlc_timestamp TEXT`).

-- Client identity: PUBLIC half only. The mnemonic lives in Stronghold.
CREATE TABLE identity_config (
    public_key TEXT PRIMARY KEY NOT NULL,
    bip39_mnemonic_verified BOOLEAN NOT NULL DEFAULT 0,
    hlc_timestamp TEXT NOT NULL
);

-- Goal: root of the milestone tree; velocity anchor.
CREATE TABLE goals (
    id TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL,
    description TEXT,
    target_date TEXT,
    status TEXT NOT NULL DEFAULT 'active'
        CHECK(status IN ('active', 'achieved', 'archived')),
    hlc_timestamp TEXT NOT NULL
);

-- Master plan milestones (PRD §7).
CREATE TABLE milestones (
    id TEXT PRIMARY KEY NOT NULL,
    goal_id TEXT NOT NULL REFERENCES goals(id),
    title TEXT NOT NULL,
    description TEXT,
    order_index INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending'
        CHECK(status IN ('pending', 'active', 'completed', 'demoted')),
    hlc_timestamp TEXT NOT NULL
);
CREATE INDEX idx_milestones_goal ON milestones(goal_id, order_index);

-- Directives: the Stackelberg queue (PRD §7) + progressive totals.
CREATE TABLE directives (
    id TEXT PRIMARY KEY NOT NULL,
    milestone_id TEXT NOT NULL REFERENCES milestones(id),
    title TEXT NOT NULL,
    execution_context TEXT,
    estimated_minutes INTEGER NOT NULL,
    progressive_step INTEGER NOT NULL DEFAULT 1,
    progressive_total INTEGER NOT NULL DEFAULT 1,
    state TEXT NOT NULL DEFAULT 'queued'
        CHECK(state IN ('queued', 'active', 'completed', 'blocked', 'skipped')),
    scheduled_for_date TEXT NOT NULL, -- YYYY-MM-DD
    hlc_timestamp TEXT NOT NULL
);
CREATE INDEX idx_directives_milestone ON directives(milestone_id);
CREATE INDEX idx_directives_state_date ON directives(state, scheduled_for_date);

-- Progressive micro-directives (PRD §5.2).
CREATE TABLE directive_phases (
    directive_id TEXT NOT NULL REFERENCES directives(id),
    step INTEGER NOT NULL,
    title TEXT NOT NULL,
    instruction TEXT,
    minutes INTEGER NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK(state IN ('pending', 'active', 'done')),
    PRIMARY KEY (directive_id, step)
);

-- Evening check-ins: objective velocity data, zero-guilt (PRD §5.4).
CREATE TABLE check_ins (
    id TEXT PRIMARY KEY NOT NULL,
    date TEXT NOT NULL UNIQUE, -- YYYY-MM-DD, one audit per day
    outcome TEXT NOT NULL CHECK(outcome IN ('done', 'partial', 'skipped')),
    note TEXT,
    hlc_timestamp TEXT NOT NULL
);

-- Escape-hatch ledger (PRD §5.3).
CREATE TABLE bailouts (
    id TEXT PRIMARY KEY NOT NULL,
    directive_id TEXT NOT NULL REFERENCES directives(id),
    reason TEXT NOT NULL
        CHECK(reason IN ('external_dependency', 'miscalculated_scope', 'energy_depletion')),
    note TEXT,
    hlc_timestamp TEXT NOT NULL
);
CREATE INDEX idx_bailouts_directive ON bailouts(directive_id);

-- Non-secret app settings (single row, id=1). API keys live in
-- Stronghold ONLY — never here, never synced.
CREATE TABLE app_settings (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    theme TEXT NOT NULL DEFAULT 'dark',
    hotkey TEXT NOT NULL DEFAULT 'alt+space',
    always_on_top BOOLEAN NOT NULL DEFAULT 0,
    ai_provider TEXT,
    tier1_model TEXT,
    tier2_model TEXT,
    relay_url TEXT
);

-- CRDT durable outbox queue (PRD §7): pending encrypted ops awaiting
-- relay push. `applied_remote` tracks whether the op was pushed.
CREATE TABLE crdt_outbox (
    operation_id TEXT PRIMARY KEY NOT NULL,
    hlc_timestamp TEXT NOT NULL,
    table_name TEXT NOT NULL,
    record_id TEXT NOT NULL,
    encrypted_payload BLOB NOT NULL, -- ChaCha20-Poly1305 ciphertext
    created_at_epoch_ms INTEGER NOT NULL,
    pushed BOOLEAN NOT NULL DEFAULT 0
);
CREATE INDEX idx_outbox_pending ON crdt_outbox(pushed, created_at_epoch_ms);

-- Applied-operation watermark: prevents re-applying remote ops we
-- already hold (idempotent apply-remote, US-4).
CREATE TABLE crdt_applied (
    operation_id TEXT PRIMARY KEY NOT NULL,
    hlc_timestamp TEXT NOT NULL
);

-- Local clock head + device discriminator for HLC resume across
-- restarts (monotonicity must survive process death).
CREATE TABLE hlc_clock (
    id INTEGER PRIMARY KEY CHECK(id = 1),
    last_wall_nanos INTEGER NOT NULL, -- SQLite INTEGER (i64)
    counter INTEGER NOT NULL DEFAULT 0,
    device INTEGER NOT NULL
);
