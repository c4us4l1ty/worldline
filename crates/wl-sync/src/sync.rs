//! Sync session: push pending outbox ops, pull and apply remote ops.
//! Transport is injected (native: reqwest; tests: in-process axum).

use wl_core::crypto::aead::{self, Sealed};
use wl_core::crypto::identity::Identity;
use wl_core::hlc::HlcTimestamp;
use wl_core::store::repo::Repos;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("store: {0}")]
    Store(#[from] wl_core::store::StoreError),
    #[error("crypto: {0}")]
    Crypto(#[from] aead::AeadError),
    #[error("transport: {0}")]
    Transport(String),
    #[error("protocol decode: {0}")]
    Protocol(String),
}
impl From<rusqlite::Error> for SyncError {
    fn from(e: rusqlite::Error) -> Self {
        SyncError::Store(e.into())
    }
}

/// Transport trait — one round-trip abstraction, mockable in tests.
pub trait Transport {
    /// Auth + execute a POST; returns response body text.
    fn post(&self, path: &str, body: &serde_json::Value) -> Result<String, String>;
}

/// Result of one full sync cycle.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncStats {
    pub pushed: usize,
    pub pulled: usize,
    pub applied: usize,
    /// Poison ops skipped-and-watermarked (undecryptable/unparseable:
    /// cannot apply, must not wedge the cursor). Surfaces in logs, not
    /// the UI pill (which tracks actionable `pending`).
    pub quarantined: usize,
    pub cursor: String,
}

/// Upper bound on push/pull batches per cycle: a malicious relay
/// answering `exhausted=false` with full batches forever must not spin
/// the client unboundedly (near-0-CPU + DoS). 200 batches × 500 ops is
/// far beyond any legitimate single-cycle backlog.
const MAX_BATCHES_PER_CYCLE: usize = 200;
/// Wire batch size clamp: ≥1 (a 0 limit would hot-loop empty pulls),
/// ≤ relay `PULL_HARD_CAP` (oversized requests are refused anyway).
const MAX_BATCH_LIMIT: usize = 500;

/// Runs one push+pull cycle against the relay.
///
/// Push: drains all pending outbox ops (encrypted at enqueue time —
/// we only re-encode to base64 for the wire). Both `accepted` AND
/// `duplicates` from the relay mark ops pushed: a push that landed
/// server-side but whose response was lost must drain on the retry,
/// not wedge the op pending forever. Pull: fetches ops after the
/// persisted `(hlc, op_id)` cursor, decrypts, applies via LWW
/// arbitration, records them in `crdt_applied` for idempotence, and
/// persists the new cursor before returning.
pub fn sync_cycle<T: Transport>(
    repos: &Repos,
    identity: &Identity,
    transport: &T,
    batch_limit: usize,
) -> Result<SyncStats, SyncError> {
    let batch_limit = batch_limit.clamp(1, MAX_BATCH_LIMIT);
    let mut pushed = 0;
    let mut pulled = 0;
    let mut applied = 0;
    let mut quarantined = 0;

    // ---- Push ----
    // P3: drain in a loop, not one batch per cycle. A large offline
    // burst (AI master plan, long offline stretch) may hold many more
    // ops than `batch_limit`; pushing once per user-triggered cycle
    // forces repeated "Sync now" clicks with the pending pill accusing
    // the user of residue the engine refused to drain.
    for _ in 0..MAX_BATCHES_PER_CYCLE {
        let pending = repos.pending_outbox(batch_limit as i64)?;
        if pending.is_empty() {
            break;
        }
        let batch_len = pending.len();
        let ops: Vec<wl_protocol::PushOp> = pending
            .iter()
            .map(|o: &wl_core::store::repo::OutboxOp| wl_protocol::PushOp {
                operation_id: o.operation_id.clone(),
                hlc: o.hlc_timestamp.to_string(),
                table: o.table_name.clone(),
                record_id: o.record_id.clone(),
                sealed_b64: base64::Engine::encode(
                    &base64::engine::general_purpose::STANDARD,
                    &o.encrypted_payload,
                ),
            })
            .collect();
        let body = serde_json::to_value(wl_protocol::PushRequest { ops })
            .map_err(|e| SyncError::Protocol(e.to_string()))?;
        let resp_text = transport
            .post("/sync/push", &body)
            .map_err(SyncError::Transport)?;
        let resp: wl_protocol::PushResponse =
            serde_json::from_str(&resp_text).map_err(|e| SyncError::Protocol(e.to_string()))?;
        // Accepted + duplicates = everything the relay now durably
        // holds from this batch. Marking only `accepted` would strand
        // ops whose earlier push response was lost.
        let mut drained = resp.accepted;
        drained.extend(resp.duplicates.iter().cloned());
        repos.mark_outbox_pushed(&drained)?;
        pushed += drained.len();
        if drained.is_empty() {
            // Relay stored nothing (validation refuse, oversized batch,
            // transient 2xx with empty lists): stop instead of hot-looping
            // the same batch forever; the residue stays pending for the
            // next cycle and surfaces via the pending count.
            break;
        }
        if batch_len < batch_limit {
            // Last partial batch: the outbox is drained.
            break;
        }
    }

    // ---- Pull ----
    let (mut cursor_hlc, mut cursor_op) = current_cursor(repos)?;
    for _ in 0..MAX_BATCHES_PER_CYCLE {
        let body = serde_json::to_value(wl_protocol::PullRequest {
            since_hlc: cursor_hlc.clone(),
            since_op_id: cursor_op.clone(),
            limit: batch_limit as u32,
        })
        .map_err(|e| SyncError::Protocol(e.to_string()))?;
        let resp_text = transport
            .post("/sync/pull", &body)
            .map_err(SyncError::Transport)?;
        let resp: wl_protocol::PullResponse =
            serde_json::from_str(&resp_text).map_err(|e| SyncError::Protocol(e.to_string()))?;
        for op in &resp.ops {
            if repos.is_op_applied(&op.operation_id)? {
                continue; // idempotent apply
            }
            match apply_pulled_op(repos, identity, op) {
                Ok(()) => applied += 1,
                Err(_) => {
                    // Poison op (bad base64, truncated sealed, wrong
                    // key/AAD, non-JSON payload, malformed HLC): it can
                    // never apply, but aborting here would wedge the
                    // cursor and brick every future pull on one crafted
                    // row. Watermark it as seen (quarantine) and advance
                    // past it — availability over a single op.
                    repos.mark_op_applied_str(&op.operation_id, &op.hlc)?;
                    quarantined += 1;
                }
            }
        }
        pulled += resp.ops.len();
        cursor_hlc = resp.next_cursor.clone();
        cursor_op = resp.next_op_id.clone();
        // Persist incrementally so a crash mid-cycle never re-pulls
        // from "" (and never skips batches already applied).
        save_cursor(repos, &cursor_hlc, &cursor_op)?;
        if resp.exhausted || resp.ops.is_empty() {
            break;
        }
    }

    // LWW arbitration above can merge `active` states from two devices
    // (each side activated while offline). Reconcile deterministically
    // so the single-directive invariant holds across devices too.
    if applied > 0 {
        let _ = repos.enforce_single_active(Some(identity));
    }

    Ok(SyncStats {
        pushed,
        pulled,
        applied,
        quarantined,
        cursor: cursor_hlc,
    })
}

/// Decrypts, parses, applies and watermarks one pulled op. Any failure
/// is terminal for the op (caller quarantines); successes also merge
/// the remote clock (HLC receive event — without it a fast peer's ops
/// would be followed by older local ticks and LWW would invert).
fn apply_pulled_op(
    repos: &Repos,
    identity: &Identity,
    op: &wl_protocol::PushOp,
) -> Result<(), SyncError> {
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &op.sealed_b64)
        .map_err(|_| SyncError::Protocol("bad base64 in pulled op".into()))?;
    let sealed = Sealed::from_bytes(&bytes)?;
    let aad = format!("{}:{}", op.table, op.record_id);
    let plaintext = aead::unseal(identity, &sealed, aad.as_bytes())?;
    let fields: serde_json::Value = serde_json::from_slice(&plaintext)
        .map_err(|e| SyncError::Protocol(format!("decrypted payload not JSON: {e}")))?;
    let ts = HlcTimestamp::parse(&op.hlc).map_err(|e| SyncError::Protocol(e.to_string()))?;
    let crdt_op = wl_core::crdt::CrdtOp {
        operation_id: op.operation_id.clone(),
        table: op.table.clone(),
        record_id: op.record_id.clone(),
        hlc: op.hlc.clone(),
        device: ts.device,
        fields: Some(fields),
        tombstone: false,
    };
    apply_op_to_db(repos, &crdt_op)?;
    repos.mark_op_applied(&op.operation_id, ts)?;
    repos.observe_remote_hlc(&ts);
    Ok(())
}

/// Applies a remote CRDT op to the local SQLite tables under LWW
/// arbitration: each upsert is guarded by `hlc_timestamp < op.ts`, so
/// an op older than the row's current state is ignored (convergent on
/// every delivery order, matching the in-memory `TableState` contract).
/// The op is still marked applied — stale ops must not be re-fetched
/// or re-arbitrated forever.
fn apply_op_to_db(repos: &Repos, op: &wl_core::crdt::CrdtOp) -> Result<(), SyncError> {
    use wl_core::crdt::CrdtTable;
    let Some(table) = CrdtTable::from_str(&op.table) else {
        return Ok(()); // unknown table: ignore deterministically
    };
    // The relay only stores ops the owner encrypted; all fields are
    // ours. Guarded upserts per table keep merge explicit and
    // deterministic; the guard IS the LWW check.
    let f = op
        .fields
        .as_ref()
        .ok_or_else(|| SyncError::Protocol("upsert op without fields".into()))?;
    let ts = HlcTimestamp::parse(&op.hlc).map_err(|e| SyncError::Protocol(e.to_string()))?;
    let ts_s = ts.to_string();
    let get = |k: &str| f[k].as_str().map(|s| s.to_string());

    let conn = repos.conn.lock().unwrap();
    match table {
        CrdtTable::Goals => {
            conn.execute(
                "INSERT INTO goals (id,title,description,target_date,status,hlc_timestamp)
                 VALUES (?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(id) DO UPDATE SET title=?2,description=?3,target_date=?4,status=?5,hlc_timestamp=?6
                 WHERE goals.hlc_timestamp < ?6",
                rusqlite::params![
                    op.record_id, get("title"), get("description"), get("target_date"),
                    get("status").unwrap_or("active".into()), ts_s
                ],
            )?;
        }
        CrdtTable::Milestones => {
            conn.execute(
                "INSERT INTO milestones (id,goal_id,title,description,order_index,status,hlc_timestamp)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(id) DO UPDATE SET goal_id=?2,title=?3,description=?4,order_index=?5,status=?6,hlc_timestamp=?7
                 WHERE milestones.hlc_timestamp < ?7",
                rusqlite::params![
                    op.record_id, get("goal_id"), get("title"), get("description"),
                    f["order_index"].as_i64().unwrap_or(0),
                    get("status").unwrap_or("pending".into()), ts_s
                ],
            )?;
        }
        CrdtTable::Directives => {
            conn.execute(
                "INSERT INTO directives (id,milestone_id,title,execution_context,estimated_minutes,
                    progressive_step,progressive_total,state,scheduled_for_date,hlc_timestamp)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
                 ON CONFLICT(id) DO UPDATE SET milestone_id=?2,title=?3,execution_context=?4,
                    estimated_minutes=?5,progressive_step=?6,progressive_total=?7,state=?8,
                    scheduled_for_date=?9,hlc_timestamp=?10
                 WHERE directives.hlc_timestamp < ?10",
                rusqlite::params![
                    op.record_id,
                    get("milestone_id"),
                    get("title"),
                    get("execution_context"),
                    f["estimated_minutes"].as_i64().unwrap_or(0),
                    f["progressive_step"].as_i64().unwrap_or(1),
                    f["progressive_total"].as_i64().unwrap_or(1),
                    get("state").unwrap_or("queued".into()),
                    get("scheduled_for_date").unwrap_or("1970-01-01".into()),
                    ts_s
                ],
            )?;
        }
        CrdtTable::DirectivePhases => {
            let step = f["step"].as_i64().unwrap_or(0);
            conn.execute(
                "INSERT INTO directive_phases (directive_id,step,title,instruction,minutes,state,hlc_timestamp)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(directive_id,step) DO UPDATE SET title=?3,instruction=?4,minutes=?5,state=?6,hlc_timestamp=?7
                 WHERE directive_phases.hlc_timestamp < ?7",
                rusqlite::params![
                    op.record_id, step, get("title"), get("instruction"),
                    f["minutes"].as_i64().unwrap_or(0),
                    get("state").unwrap_or("pending".into()), ts_s
                ],
            )?;
        }
        CrdtTable::CheckIns => {
            conn.execute(
                "INSERT INTO check_ins (id,date,outcome,note,hlc_timestamp)
                 VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(id) DO UPDATE SET date=?2,outcome=?3,note=?4,hlc_timestamp=?5
                 WHERE check_ins.hlc_timestamp < ?5",
                rusqlite::params![op.record_id, get("date"), get("outcome"), get("note"), ts_s],
            )?;
        }
        CrdtTable::Bailouts => {
            conn.execute(
                "INSERT INTO bailouts (id,directive_id,reason,note,hlc_timestamp)
                 VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(id) DO UPDATE SET directive_id=?2,reason=?3,note=?4,hlc_timestamp=?5
                 WHERE bailouts.hlc_timestamp < ?5",
                rusqlite::params![
                    op.record_id,
                    get("directive_id"),
                    get("reason"),
                    get("note"),
                    ts_s
                ],
            )?;
        }
        CrdtTable::AppSettings => {
            conn.execute(
                "INSERT INTO app_settings (id,theme,hotkey,always_on_top,ai_provider,tier1_model,tier2_model,relay_url,hlc_timestamp)
                 VALUES (1,?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(id) DO UPDATE SET theme=?1,hotkey=?2,always_on_top=?3,ai_provider=?4,tier1_model=?5,tier2_model=?6,relay_url=?7,hlc_timestamp=?8
                 WHERE app_settings.hlc_timestamp < ?8",
                rusqlite::params![
                    get("theme").unwrap_or("dark".into()), get("hotkey").unwrap_or("alt+space".into()),
                    f["always_on_top"].as_i64().unwrap_or(0),
                    get("ai_provider"), get("tier1_model"), get("tier2_model"), get("relay_url"),
                    ts_s
                ],
            )?;
        }
    }
    Ok(())
}

/// Reads the persisted pull cursor `(hlc, op_id)` ("" / "" = start).
fn current_cursor(repos: &Repos) -> Result<(String, String), SyncError> {
    let conn = repos.conn.lock().unwrap();
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT cursor, op_id FROM sync_cursor WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.unwrap_or_default())
}

/// Persists the pull cursor (idempotent; called by `sync_cycle`
/// itself so the shell cannot forget).
pub fn save_cursor(repos: &Repos, cursor: &str, op_id: &str) -> Result<(), SyncError> {
    let conn = repos.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO sync_cursor (id, cursor, op_id) VALUES (1, ?1, ?2)
         ON CONFLICT(id) DO UPDATE SET cursor=?1, op_id=?2",
        rusqlite::params![cursor, op_id],
    )?;
    Ok(())
}

/// HLC receive event against the shell's real clock (persists the
/// head). Previously this built a throwaway clock per call and never
/// advanced anything — a causality no-op. Fixed to merge into
/// `repos.hlc` like the pull path does.
pub fn observe_remote(repos: &Repos, remote: &HlcTimestamp) -> HlcTimestamp {
    repos.observe_remote_hlc(remote)
}

/// `Option` extension used by [`current_cursor`].
use rusqlite::OptionalExtension;
