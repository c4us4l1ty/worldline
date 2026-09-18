//! Sync session: push pending outbox ops, pull and apply remote ops.
//! Transport is injected (native: reqwest; tests: in-process axum).

use std::collections::HashSet;

use rusqlite::OptionalExtension;
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
const MAX_BATCH_LIMIT: usize = wl_protocol::MAX_BATCH_OPS;

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
        let resp: wl_protocol::PushResponse = decode_response(&resp_text)?;
        let sent: HashSet<&str> = pending.iter().map(|o| o.operation_id.as_str()).collect();
        let mut acknowledged = HashSet::new();
        for id in resp.accepted.iter().chain(&resp.duplicates) {
            if !sent.contains(id.as_str()) || !acknowledged.insert(id.as_str()) {
                return Err(SyncError::Protocol("invalid push acknowledgement".into()));
            }
        }
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
        if drained.len() < batch_len || batch_len < batch_limit {
            // Last partial batch: the outbox is drained.
            break;
        }
    }
    // Every drained op was acknowledged as relay-durable (accepted or
    // duplicate): delete instead of accumulating `pushed = 1` rows
    // forever on long-lived installs. Unconditional (idempotent), so a
    // crash between mark and delete never leaves residue either.
    repos.delete_pushed_outbox()?;

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
        let resp: wl_protocol::PullResponse = decode_response(&resp_text)?;
        validate_pull_response(&resp, &cursor_hlc, &cursor_op, batch_limit)?;
        for op in &resp.ops {
            // B-008: a future-schema table must quarantine-skip
            // (watermark + advance) like any poison op — never abort
            // the cycle and wedge every future pull behind it.
            if wl_core::crdt::CrdtTable::from_str(&op.table).is_none() {
                repos.mark_op_applied_str(&op.operation_id, &op.hlc)?;
                quarantined += 1;
                continue;
            }
            if repos.is_op_applied(&op.operation_id)? {
                continue; // idempotent apply
            }
            match apply_pulled_op(repos, identity, op) {
                Ok(()) => applied += 1,
                Err(e) if is_fatal_store_error(&e) => return Err(e),
                Err(_) => {
                    // Poison op (bad base64, truncated sealed, wrong
                    // key/AAD, non-JSON payload, malformed HLC) AND
                    // constraint conflicts (a cross-row UNIQUE or FK clash
                    // from a same-id/different-date row move): neither can
                    // ever apply, but aborting here would wedge the
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

    // The relay paginates strictly after the cursor, so watermarks at or
    // below it can never match a future pull: prune instead of growing
    // `crdt_applied` forever.
    if !cursor_hlc.is_empty() {
        repos.prune_applied_below(&cursor_hlc, &cursor_op)?;
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
    // Tombstone (delete) ops carry the sealed marker instead of row
    // fields (B-003); anything else is an upsert.
    let tombstone = fields
        .get(wl_core::crdt::TOMBSTONE_MARKER)
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let crdt_op = wl_core::crdt::CrdtOp {
        operation_id: op.operation_id.clone(),
        table: op.table.clone(),
        record_id: op.record_id.clone(),
        hlc: op.hlc.clone(),
        device: ts.device,
        fields: if tombstone { None } else { Some(fields) },
        tombstone,
    };
    apply_op_to_db(repos, &crdt_op)?;
    repos.mark_op_applied(&op.operation_id, ts)?;
    repos.observe_remote_hlc(&ts);
    Ok(())
}

/// Applies a remote CRDT op to the local SQLite tables under total LWW
/// arbitration, mirroring the in-memory `TableState` contract: every op
/// is ordered by `(hlc, device, operation_id)` against the per-record
/// merge head in `record_heads`, so equal-HLC races (forked data dirs
/// sharing a device id and clock head) converge identically on every
/// replica instead of order-dependently (B-009). Tombstone ops delete
/// their row and plant delete memory, so older upserts arriving later
/// cannot resurrect it (B-003); a strictly newer upsert clears the
/// tombstone and resurrects, exactly like `TableState`.
///
/// The op is marked applied by the caller either way — stale ops must
/// not be re-fetched or re-arbitrated forever.
fn apply_op_to_db(repos: &Repos, op: &wl_core::crdt::CrdtOp) -> Result<(), SyncError> {
    use wl_core::crdt::CrdtTable;
    let Some(table) = CrdtTable::from_str(&op.table) else {
        return Ok(()); // unknown table: ignore deterministically (B-008 quarantines in the loop)
    };
    // Field-less non-tombstone ops are malformed (crafted/truncated):
    // reject before touching any head state (CORE-5 adjacent).
    if !op.tombstone && op.fields.is_none() {
        return Err(SyncError::Protocol("upsert op without fields".into()));
    }
    let ts = HlcTimestamp::parse(&op.hlc).map_err(|e| SyncError::Protocol(e.to_string()))?;
    let ts_s = ts.to_string();
    let get = |f: &serde_json::Value, k: &str| f[k].as_str().map(|s| s.to_string());

    let conn = repos.conn.lock().unwrap();
    let head = load_head(&conn, table.as_str(), &op.record_id)?;
    // Effective head: stored merge memory, else the live row's own HLC
    // (legacy/local rows predate heads — they lose exact ties, remote
    // wins), else unseen (accept anything).
    let (h_hlc, h_dev, h_op, h_tomb) = match head {
        Some(h) => (h.hlc, h.device, h.op_id, h.tombstone),
        None => match row_hlc(&conn, table, &op.record_id)? {
            Some(row_hlc) => (row_hlc, 0, String::new(), false),
            None => (String::new(), 0, String::new(), false),
        },
    };
    // Op wins iff its key strictly beats the head (total order).
    if ts_s < h_hlc
        || (ts_s == h_hlc && (ts.device < h_dev || (ts.device == h_dev && op.operation_id <= h_op)))
    {
        return Ok(()); // stale: ignore deterministically
    }
    if op.tombstone {
        apply_delete(&conn, table, &op.record_id)?;
        store_head(&conn, table.as_str(), &op.record_id, &ts_s, ts.device, &op.operation_id, true)?;
        return Ok(());
    }
    let f = op.fields.as_ref().unwrap();
    if h_tomb && ts_s == h_hlc {
        // Matches `TableState`: an op that only beats a tombstone via
        // the device/op tie-break does not resurrect — advance the head
        // but keep delete memory.
        store_head(&conn, table.as_str(), &op.record_id, &ts_s, ts.device, &op.operation_id, true)?;
        return Ok(());
    }
    if table == CrdtTable::CheckIns {
        if !apply_check_in_win(&conn, &op.record_id, f, &ts_s, ts.device, &op.operation_id)? {
            return Ok(()); // lost the date to a newer same-day row
        }
    } else {
        apply_upsert(&conn, table, &op.record_id, f, &ts_s, &get)?;
    }
    store_head(&conn, table.as_str(), &op.record_id, &ts_s, ts.device, &op.operation_id, false)?;
    Ok(())
}

/// Merge head for one record: the greatest `(hlc, device, op_id)` key
/// applied so far, plus whether that winner was a tombstone.
struct Head {
    hlc: String,
    device: u16,
    op_id: String,
    tombstone: bool,
}

fn load_head(
    conn: &rusqlite::Connection,
    table: &str,
    record: &str,
) -> Result<Option<Head>, SyncError> {
    let row: Option<(String, i64, String, i64)> = conn
        .query_row(
            "SELECT hlc_timestamp, device, operation_id, tombstone FROM record_heads
             WHERE table_name = ?1 AND record_id = ?2",
            rusqlite::params![table, record],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    Ok(row.map(|(hlc, device, op_id, tombstone)| Head {
        hlc,
        device: device as u16,
        op_id,
        tombstone: tombstone != 0,
    }))
}

fn store_head(
    conn: &rusqlite::Connection,
    table: &str,
    record: &str,
    hlc: &str,
    device: u16,
    op_id: &str,
    tombstone: bool,
) -> Result<(), SyncError> {
    conn.execute(
        "INSERT INTO record_heads (table_name, record_id, hlc_timestamp, device, operation_id, tombstone)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(table_name, record_id) DO UPDATE SET hlc_timestamp=?3, device=?4, operation_id=?5, tombstone=?6",
        rusqlite::params![table, record, hlc, device as i64, op_id, tombstone as i64],
    )?;
    Ok(())
}

/// Fallback HLC for rows with no stored head (pre-migration or local
/// writes, which don't maintain heads).
fn row_hlc(
    conn: &rusqlite::Connection,
    table: wl_core::crdt::CrdtTable,
    record: &str,
) -> Result<Option<String>, SyncError> {
    use wl_core::crdt::CrdtTable;
    let hlc: Option<String> = match table {
        CrdtTable::Goals => conn
            .query_row("SELECT hlc_timestamp FROM goals WHERE id = ?1", [record], |r| {
                r.get(0)
            })
            .optional()?,
        CrdtTable::Milestones => conn
            .query_row("SELECT hlc_timestamp FROM milestones WHERE id = ?1", [record], |r| {
                r.get(0)
            })
            .optional()?,
        CrdtTable::Directives => conn
            .query_row("SELECT hlc_timestamp FROM directives WHERE id = ?1", [record], |r| {
                r.get(0)
            })
            .optional()?,
        // Phase ops share the directive-level record id (one op per
        // step): the coarsest live phase HLC is the fallback.
        CrdtTable::DirectivePhases => conn
            .query_row(
                "SELECT MAX(hlc_timestamp) FROM directive_phases WHERE directive_id = ?1",
                [record],
                |r| r.get(0),
            )
            .optional()?
            .flatten(),
        CrdtTable::CheckIns => conn
            .query_row("SELECT hlc_timestamp FROM check_ins WHERE id = ?1", [record], |r| {
                r.get(0)
            })
            .optional()?,
        CrdtTable::Bailouts => conn
            .query_row("SELECT hlc_timestamp FROM bailouts WHERE id = ?1", [record], |r| {
                r.get(0)
            })
            .optional()?,
        CrdtTable::AppSettings => conn
            .query_row("SELECT hlc_timestamp FROM app_settings WHERE id = 1", [], |r| {
                r.get(0)
            })
            .optional()?,
    };
    Ok(hlc)
}

/// Plain upsert for single-PK tables. No SQL-side guard: the caller
/// already arbitrated against the merge head, so the write is final.
fn apply_upsert(
    conn: &rusqlite::Connection,
    table: wl_core::crdt::CrdtTable,
    record_id: &str,
    f: &serde_json::Value,
    ts_s: &str,
    get: &dyn Fn(&serde_json::Value, &str) -> Option<String>,
) -> Result<(), SyncError> {
    use wl_core::crdt::CrdtTable;
    match table {
        CrdtTable::Goals => {
            conn.execute(
                "INSERT INTO goals (id,title,description,target_date,status,hlc_timestamp)
                 VALUES (?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(id) DO UPDATE SET title=?2,description=?3,target_date=?4,status=?5,hlc_timestamp=?6",
                rusqlite::params![
                    record_id, get(f, "title"), get(f, "description"), get(f, "target_date"),
                    get(f, "status").unwrap_or("active".into()), ts_s
                ],
            )?;
        }
        CrdtTable::Milestones => {
            conn.execute(
                "INSERT INTO milestones (id,goal_id,title,description,order_index,status,hlc_timestamp)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(id) DO UPDATE SET goal_id=?2,title=?3,description=?4,order_index=?5,status=?6,hlc_timestamp=?7",
                rusqlite::params![
                    record_id, get(f, "goal_id"), get(f, "title"), get(f, "description"),
                    f["order_index"].as_i64().unwrap_or(0),
                    get(f, "status").unwrap_or("pending".into()), ts_s
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
                    scheduled_for_date=?9,hlc_timestamp=?10",
                rusqlite::params![
                    record_id,
                    get(f, "milestone_id"),
                    get(f, "title"),
                    get(f, "execution_context"),
                    f["estimated_minutes"].as_i64().unwrap_or(0),
                    f["progressive_step"].as_i64().unwrap_or(1),
                    f["progressive_total"].as_i64().unwrap_or(1),
                    get(f, "state").unwrap_or("queued".into()),
                    get(f, "scheduled_for_date").unwrap_or("1970-01-01".into()),
                    ts_s
                ],
            )?;
        }
        CrdtTable::DirectivePhases => {
            let step = f["step"].as_i64().unwrap_or(0);
            conn.execute(
                "INSERT INTO directive_phases (directive_id,step,title,instruction,minutes,state,hlc_timestamp)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(directive_id,step) DO UPDATE SET title=?3,instruction=?4,minutes=?5,state=?6,hlc_timestamp=?7",
                rusqlite::params![
                    record_id, step, get(f, "title"), get(f, "instruction"),
                    f["minutes"].as_i64().unwrap_or(0),
                    get(f, "state").unwrap_or("pending".into()), ts_s
                ],
            )?;
        }
        // Check-ins merge through the date-aware winner-take-all path;
        // the caller routes them there and never reaches this arm.
        CrdtTable::CheckIns => {}
        CrdtTable::Bailouts => {
            conn.execute(
                "INSERT INTO bailouts (id,directive_id,reason,note,hlc_timestamp)
                 VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(id) DO UPDATE SET directive_id=?2,reason=?3,note=?4,hlc_timestamp=?5",
                rusqlite::params![
                    record_id,
                    get(f, "directive_id"),
                    get(f, "reason"),
                    get(f, "note"),
                    ts_s
                ],
            )?;
        }
        CrdtTable::AppSettings => {
            conn.execute(
                "INSERT INTO app_settings (id,theme,hotkey,always_on_top,ai_provider,tier1_model,tier2_model,relay_url,hlc_timestamp)
                 VALUES (1,?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(id) DO UPDATE SET theme=?1,hotkey=?2,always_on_top=?3,ai_provider=?4,tier1_model=?5,tier2_model=?6,relay_url=?7,hlc_timestamp=?8",
                rusqlite::params![
                    get(f, "theme").unwrap_or("dark".into()), get(f, "hotkey").unwrap_or("alt+space".into()),
                    f["always_on_top"].as_i64().unwrap_or(0),
                    get(f, "ai_provider"), get(f, "tier1_model"), get(f, "tier2_model"), get(f, "relay_url"),
                    ts_s
                ],
            )?;
        }
    }
    Ok(())
}

/// Check-in merge: one audit per day, so an op can conflict on `id`
/// AND on `date` (same-day re-check-in from another device with a new
/// id). Winner-take-all on the full arbitration key across every row
/// sharing the date: the op writes iff its key beats all of them.
/// Returns `true` when the op won the date (row written). Evicted
/// same-date losers get delete memory at the winner's key so their
/// older ops cannot resurrect ghosts; their newer ops still arbitrate
/// normally and can retake the date later.
fn apply_check_in_win(
    conn: &rusqlite::Connection,
    record_id: &str,
    f: &serde_json::Value,
    ts_s: &str,
    device: u16,
    op_id: &str,
) -> Result<bool, SyncError> {
    let get = |k: &str| f[k].as_str().map(|s| s.to_string());
    let date = get("date").unwrap_or_default();
    // Key of every row sharing the date: stored head, else row HLC.
    let sharers: Vec<(String, String, u16, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, hlc_timestamp FROM check_ins WHERE date = ?1",
        )?;
        let rows = stmt
            .query_map(rusqlite::params![date], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut out = Vec::with_capacity(rows.len());
        for (id, row_hlc) in rows {
            match load_head(conn, "check_ins", &id)? {
                Some(h) => out.push((id, h.hlc, h.device, h.op_id)),
                None => out.push((id, row_hlc, 0, String::new())),
            }
        }
        out
    };
    let beats = |key: &(String, u16, String)| {
        ts_s > key.0.as_str()
            || (ts_s == key.0.as_str()
                && (device > key.1 || (device == key.1 && op_id > key.2.as_str())))
    };
    if sharers
        .iter()
        .any(|(id, hlc, dev, op)| id != record_id && !beats(&(hlc.clone(), *dev, op.clone())))
    {
        return Ok(false); // a newer same-day row owns the date
    }
    for (id, _, _, _) in sharers.iter().filter(|(id, _, _, _)| id != record_id) {
        conn.execute("DELETE FROM check_ins WHERE id = ?1", [id])?;
        store_head(conn, "check_ins", id, ts_s, device, op_id, true)?;
    }
    conn.execute(
        "INSERT INTO check_ins (id,date,outcome,note,hlc_timestamp)
         VALUES (?1,?2,?3,?4,?5)
         ON CONFLICT(id) DO UPDATE SET date=?2,outcome=?3,note=?4,hlc_timestamp=?5",
        rusqlite::params![record_id, get("date"), get("outcome"), get("note"), ts_s],
    )?;
    Ok(true)
}

/// Tombstone apply: remove the row (phases: all steps of the
/// directive — matching the directive-level record id). Missing rows
/// are fine (idempotent); FK-blocked deletes (children still present)
/// surface as constraint errors into the existing quarantine path —
/// writers delete leaf-first.
fn apply_delete(
    conn: &rusqlite::Connection,
    table: wl_core::crdt::CrdtTable,
    record_id: &str,
) -> Result<(), SyncError> {
    use wl_core::crdt::CrdtTable;
    match table {
        CrdtTable::Goals => {
            conn.execute("DELETE FROM goals WHERE id = ?1", [record_id])?;
        }
        CrdtTable::Milestones => {
            conn.execute("DELETE FROM milestones WHERE id = ?1", [record_id])?;
        }
        CrdtTable::Directives => {
            conn.execute("DELETE FROM directives WHERE id = ?1", [record_id])?;
        }
        CrdtTable::DirectivePhases => {
            conn.execute("DELETE FROM directive_phases WHERE directive_id = ?1", [record_id])?;
        }
        CrdtTable::CheckIns => {
            conn.execute("DELETE FROM check_ins WHERE id = ?1", [record_id])?;
        }
        CrdtTable::Bailouts => {
            conn.execute("DELETE FROM bailouts WHERE id = ?1", [record_id])?;
        }
        // The settings singleton is never deleted; a tombstone for it
        // is a crafted op — drop it deterministically.
        CrdtTable::AppSettings => {}
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

/// Decodes a JSON response body under a hard byte bound — a hostile
/// relay answering with a multi-gigabyte body must fail closed instead
/// of exhausting client memory.
fn decode_response<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, SyncError> {
    if text.len() > wl_protocol::MAX_RESPONSE_BYTES {
        return Err(SyncError::Protocol("response exceeds size bound".into()));
    }
    serde_json::from_str(text).map_err(|e| SyncError::Protocol(e.to_string()))
}

/// Validates a pull batch against the relay's own hard caps before any
/// op is applied: batch size, per-op envelope bounds (including the
/// sealed-payload size and table allow-list — the same bounds push
/// enforces server-side), monotonic (hlc, op_id) pagination, cursor
/// continuity, and empty-batch cursor invariance. A hostile relay must
/// neither wedge the cycle nor advance the cursor past data the client
/// never received (an empty batch that jumps the cursor would make
/// every op between the old and new position permanently unreachable).
fn validate_pull_response(
    resp: &wl_protocol::PullResponse,
    since_hlc: &str,
    since_op_id: &str,
    batch_limit: usize,
) -> Result<(), SyncError> {
    if resp.ops.len() > batch_limit {
        return Err(SyncError::Protocol(
            "pull batch exceeds requested limit".into(),
        ));
    }
    if !wl_protocol::valid_cursor(&resp.next_cursor, &resp.next_op_id) {
        return Err(SyncError::Protocol("invalid next cursor".into()));
    }
    if resp.ops.is_empty() {
        if resp.next_cursor != since_hlc || resp.next_op_id != since_op_id {
            return Err(SyncError::Protocol(
                "empty batch must not advance the pull cursor".into(),
            ));
        }
        return Ok(());
    }
    let mut prev = (since_hlc.to_string(), since_op_id.to_string());
    for op in &resp.ops {
        // NOTE: no table allow-list here (B-008). Unknown tables from
        // future-schema peers quarantine-skip in the apply loop above;
        // rejecting them here would abort the cycle BEFORE any apply
        // and wedge sync. Envelope bounds still hold for every op.
        if op.operation_id.is_empty()
            || op.operation_id.len() > wl_protocol::MAX_HEADER_LEN
            || !wl_protocol::valid_hlc(&op.hlc)
            || op.record_id.is_empty()
            || op.record_id.len() > wl_protocol::MAX_HEADER_LEN
            || op.sealed_b64.len() > wl_protocol::MAX_SEALED_B64
            || op.table.is_empty()
            || op.table.len() > wl_protocol::MAX_HEADER_LEN
        {
            return Err(SyncError::Protocol(
                "pulled op envelope out of bounds".into(),
            ));
        }
        if op.hlc < prev.0 || (op.hlc == prev.0 && op.operation_id <= prev.1) {
            return Err(SyncError::Protocol("non-monotonic pull batch".into()));
        }
        prev = (op.hlc.clone(), op.operation_id.clone());
    }
    if resp.next_cursor != prev.0 || resp.next_op_id != prev.1 {
        return Err(SyncError::Protocol("cursor does not match last op".into()));
    }
    Ok(())
}

/// `true` for store failures that must abort the cycle (I/O, locks,
/// corruption, schema): retrying later may succeed. Constraint
/// violations (UNIQUE/FK clashes from adversarial or cross-row row
/// moves) are deterministic per op — retrying changes nothing — so
/// they quarantine like any other poison op instead of wedging sync.
fn is_fatal_store_error(e: &SyncError) -> bool {
    match e {
        SyncError::Store(wl_core::store::StoreError::Sqlite(db)) => !matches!(
            db,
            rusqlite::Error::SqliteFailure(err, _)
                if err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_CHECK
                    || err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY
                    || err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
                    || err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
        ),
        SyncError::Store(_) => true,
        _ => false,
    }
}
