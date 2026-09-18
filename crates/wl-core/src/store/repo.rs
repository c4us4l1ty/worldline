//! Repositories for every domain aggregate. Each write path also
//! enqueues the corresponding CRDT op into `crdt_outbox` (encrypted
//! with the identity's payload key) so sync is durable-by-default.

use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::crypto::aead;
use crate::crypto::identity::Identity;
use crate::domain::*;
use crate::hlc::{Hlc, HlcTimestamp};

use super::StoreError;

/// Repository handle. The SQLite connection lives behind a mutex so
/// `Repos` is `Send + Sync` (rusqlite `Connection` is `Send` but not
/// `Sync`) — required for sharing across Tauri's async runtime and
/// future mobile targets. All locks are statement-scoped: no guard is
/// ever held across a nested `Repos` call, so no deadlock is possible.
pub struct Repos {
    pub conn: Mutex<Connection>,
    pub hlc: Hlc,
    device: u16,
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4().simple())
}

impl Repos {
    pub fn new(conn: Connection, device: u16) -> Self {
        let repos = Self {
            conn: Mutex::new(conn),
            hlc: Hlc::new(),
            device,
        };
        // E5: resume the persisted clock head so a restart under a
        // regressed wall clock cannot issue timestamps older than
        // pre-restart ops (LWW would otherwise invert).
        if let Ok(Some((wall, ctr))) = repos.read_hlc_head() {
            repos.hlc.restore(wall, ctr);
            // Clone-split jitter: two live replicas forked from one data
            // dir (file copy, VM snapshot) share device id AND clock head,
            // so under a regressed wall clock their first ticks would stamp
            // IDENTICAL HLCs on different content and fork LWW permanently
            // (strict-`<` guards drop both remote ops). A small per-boot
            // random counter advance separates the heads with probability
            // 255/256; when the wall clock is healthy the next tick is
            // wall-based and the jitter is invisible.
            let jitter: u16 = rand::Rng::gen_range(&mut rand::thread_rng(), 0..256);
            if jitter > 0 {
                repos.hlc.restore(wall, ctr.saturating_add(jitter));
            }
        }
        repos
    }

    /// Issues a timestamp and persists the new clock head to `hlc_clock`
    /// (E5). All mutation paths go through here instead of calling
    /// `self.hlc.now` directly, so every process boot resumes at least
    /// at the last-issued head.
    fn tick(&self) -> HlcTimestamp {
        let ts = self.hlc.now(self.device);
        // Best-effort: persistence must never fail a domain write; a
        // missed head only risks monotonicity after a crash + clock
        // regression, never correctness of the current op.
        let _ = self.write_hlc_head();
        ts
    }

    fn read_hlc_head(&self) -> Result<Option<(u64, u16)>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let row: Option<(i64, i64, i64)> = conn
            .query_row(
                "SELECT last_wall_nanos, counter, device FROM hlc_clock WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(StoreError::Sqlite)?;
        Ok(row.map(|(w, c, _)| (w as u64, c as u16)))
    }

    fn write_hlc_head(&self) -> Result<(), StoreError> {
        let conn = self.conn.lock().unwrap();
        let (wall, ctr) = self.hlc.head();
        conn.execute(
            "INSERT INTO hlc_clock (id, last_wall_nanos, counter, device) VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET last_wall_nanos=?1, counter=?2, device=?3",
            params![wall as i64, ctr as i64, self.device as i64],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Identity
    // ------------------------------------------------------------------

    /// Persists the public identity half after onboarding.
    /// `verify_indices`: the 3-word backup-challenge positions, so the
    /// authoritative re-check stays positional across restarts.
    pub fn insert_identity(
        &self,
        identity: &Identity,
        verified: bool,
        verify_indices: &[usize],
    ) -> Result<(), StoreError> {
        let ts = self.tick();
        self.conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO identity_config (public_key, bip39_mnemonic_verified, verify_indices, hlc_timestamp)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(public_key) DO UPDATE SET
                    bip39_mnemonic_verified = excluded.bip39_mnemonic_verified,
                    verify_indices = excluded.verify_indices,
                    hlc_timestamp = excluded.hlc_timestamp",
                params![
                    identity.account_id_hex(),
                    verified,
                    serde_json::to_string(verify_indices).expect("json indices"),
                    ts.to_string()
                ],
            )
            .map_err(StoreError::Sqlite)?;
        Ok(())
    }

    pub fn identity(&self) -> Result<Option<IdentityConfig>, StoreError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT public_key, bip39_mnemonic_verified, verify_indices, hlc_timestamp
                 FROM identity_config LIMIT 1",
                [],
                |r| {
                    Ok(IdentityConfig {
                        public_key: r.get(0)?,
                        bip39_mnemonic_verified: r.get(1)?,
                        verify_indices: serde_json::from_str(&r.get::<_, String>(2)?)
                            .unwrap_or_default(),
                        hlc_timestamp: parse_hlc(&r.get::<_, String>(3)?)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::Sqlite)
    }

    /// Persists the backup-challenge verification result.
    pub fn set_mnemonic_verified(&self, verified: bool) -> Result<(), StoreError> {
        let ts = self.tick();
        self.conn.lock().unwrap().execute(
            "UPDATE identity_config SET bip39_mnemonic_verified = ?1, hlc_timestamp = ?2",
            params![verified, ts.to_string()],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Goals
    // ------------------------------------------------------------------

    pub fn create_goal(
        &self,
        title: &str,
        description: Option<&str>,
        target_date: Option<&str>,
        identity: Option<&Identity>,
    ) -> Result<Goal, StoreError> {
        check_text("goal title", title, MAX_TITLE_CHARS).map_err(StoreError::Invalid)?;
        check_optional_text("goal description", description, MAX_DESCRIPTION_CHARS)
            .map_err(StoreError::Invalid)?;
        if let Some(d) = target_date {
            check_date("goal target date", d).map_err(StoreError::Invalid)?;
        }
        let id = new_id("goal");
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let ts = self.hlc.now(self.device);
        let goal = Goal {
            id: id.clone(),
            title: title.to_string(),
            description: description.map(Into::into),
            target_date: target_date.map(Into::into),
            status: GoalStatus::Active,
            hlc_timestamp: ts,
        };
        tx.execute(
            "INSERT INTO goals (id, title, description, target_date, status, hlc_timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                id,
                title,
                description,
                target_date,
                "active",
                ts.to_string()
            ],
        )?;
        Self::emit_on(
            &tx,
            identity,
            crate::crdt::CrdtTable::Goals,
            &goal.id,
            &Self::goal_json(&goal),
            ts,
        )?;
        self.persist_head_on(&tx)?;
        tx.commit()?;
        Ok(goal)
    }

    pub fn active_goal(&self) -> Result<Option<Goal>, StoreError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT id, title, description, target_date, status, hlc_timestamp
                 FROM goals WHERE status = 'active' ORDER BY hlc_timestamp LIMIT 1",
                [],
                goal_row,
            )
            .optional()
            .map_err(StoreError::Sqlite)
    }

    pub fn goal(&self, id: &str) -> Result<Option<Goal>, StoreError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT id, title, description, target_date, status, hlc_timestamp
                 FROM goals WHERE id = ?1",
                [id],
                goal_row,
            )
            .optional()
            .map_err(StoreError::Sqlite)
    }

    // ------------------------------------------------------------------
    // Milestones
    // ------------------------------------------------------------------

    pub fn create_milestone(
        &self,
        goal_id: &str,
        title: &str,
        description: Option<&str>,
        order_index: i64,
        identity: Option<&Identity>,
    ) -> Result<Milestone, StoreError> {
        check_text("milestone title", title, MAX_TITLE_CHARS).map_err(StoreError::Invalid)?;
        check_optional_text("milestone description", description, MAX_DESCRIPTION_CHARS)
            .map_err(StoreError::Invalid)?;
        let id = new_id("ms");
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let ts = self.hlc.now(self.device);
        let m = Milestone {
            id: id.clone(),
            goal_id: goal_id.to_string(),
            title: title.to_string(),
            description: description.map(Into::into),
            order_index,
            status: MilestoneStatus::Pending,
            hlc_timestamp: ts,
        };
        tx.execute(
            "INSERT INTO milestones (id, goal_id, title, description, order_index, status, hlc_timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, goal_id, title, description, order_index, "pending", ts.to_string()],
        )?;
        Self::emit_on(
            &tx,
            identity,
            crate::crdt::CrdtTable::Milestones,
            &m.id,
            &Self::milestone_json(&m),
            ts,
        )?;
        self.persist_head_on(&tx)?;
        tx.commit()?;
        Ok(m)
    }

    /// All milestones of a goal, ordered.
    pub fn milestones_for_goal(&self, goal_id: &str) -> Result<Vec<Milestone>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, goal_id, title, description, order_index, status, hlc_timestamp
             FROM milestones WHERE goal_id = ?1 ORDER BY order_index",
        )?;
        let rows = stmt
            .query_map([goal_id], milestone_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_milestone_status(
        &self,
        id: &str,
        status: MilestoneStatus,
        identity: Option<&Identity>,
    ) -> Result<(), StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let ts = self.hlc.now(self.device);
        if tx.execute(
            "UPDATE milestones SET status = ?2, hlc_timestamp = ?3 WHERE id = ?1",
            params![id, status.as_str(), ts.to_string()],
        )? == 0
        {
            return Err(StoreError::NotFound(format!("milestone {id}")));
        }
        if identity.is_some() {
            let m = tx.query_row(
                "SELECT id, goal_id, title, description, order_index, status, hlc_timestamp
                 FROM milestones WHERE id = ?1",
                [id],
                milestone_row,
            )?;
            Self::emit_on(
                &tx,
                identity,
                crate::crdt::CrdtTable::Milestones,
                &m.id,
                &Self::milestone_json(&m),
                ts,
            )?;
        }
        self.persist_head_on(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// The next `pending` milestone (lowest order_index) for a goal.
    pub fn next_pending_milestone(&self, goal_id: &str) -> Result<Option<Milestone>, StoreError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT id, goal_id, title, description, order_index, status, hlc_timestamp
                 FROM milestones WHERE goal_id = ?1 AND status = 'pending'
                 ORDER BY order_index LIMIT 1",
                [goal_id],
                milestone_row,
            )
            .optional()
            .map_err(StoreError::Sqlite)
    }

    /// Single milestone by id (used by status-change write-through).
    pub fn milestone(&self, id: &str) -> Result<Option<Milestone>, StoreError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT id, goal_id, title, description, order_index, status, hlc_timestamp
                 FROM milestones WHERE id = ?1",
                [id],
                milestone_row,
            )
            .optional()
            .map_err(StoreError::Sqlite)
    }

    // ------------------------------------------------------------------
    // Directives
    // ------------------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    pub fn create_directive(
        &self,
        milestone_id: &str,
        title: &str,
        execution_context: Option<&str>,
        estimated_minutes: i64,
        progressive_total: i64,
        scheduled_for_date: &str,
        phases: &[(String, Option<String>, i64)], // (title, instruction, minutes)
        identity: Option<&Identity>,
    ) -> Result<Directive, StoreError> {
        check_text("directive title", title, MAX_TITLE_CHARS).map_err(StoreError::Invalid)?;
        check_optional_text("directive context", execution_context, MAX_CONTEXT_CHARS)
            .map_err(StoreError::Invalid)?;
        check_minutes("directive estimate", estimated_minutes).map_err(StoreError::Invalid)?;
        check_date("directive date", scheduled_for_date).map_err(StoreError::Invalid)?;
        for (pt, pi, pm) in phases {
            check_text("phase title", pt, MAX_TITLE_CHARS).map_err(StoreError::Invalid)?;
            check_optional_text("phase instruction", pi.as_deref(), MAX_CONTEXT_CHARS)
                .map_err(StoreError::Invalid)?;
            check_minutes("phase minutes", *pm).map_err(StoreError::Invalid)?;
        }
        if estimated_minutes <= 0 || progressive_total < 1 || phases.iter().any(|p| p.2 <= 0) {
            return Err(StoreError::Invalid(
                "minutes and phase count must be positive".into(),
            ));
        }
        if (progressive_total == 1 && !phases.is_empty())
            || (progressive_total > 1 && phases.len() as i64 != progressive_total)
        {
            return Err(StoreError::Invalid(format!(
                "progressive_total={progressive_total} but {} phases supplied",
                phases.len()
            )));
        }
        let id = new_id("dir");
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let ts = self.hlc.now(self.device);
        let d = Directive {
            id: id.clone(),
            milestone_id: milestone_id.to_string(),
            title: title.to_string(),
            execution_context: execution_context.map(Into::into),
            estimated_minutes,
            progressive_step: 1,
            progressive_total,
            state: DirectiveState::Queued,
            scheduled_for_date: scheduled_for_date.to_string(),
            hlc_timestamp: ts,
        };
        tx.execute(
            "INSERT INTO directives (id, milestone_id, title, execution_context,
                estimated_minutes, progressive_step, progressive_total, state, scheduled_for_date, hlc_timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, 'queued', ?7, ?8)",
            params![id, milestone_id, title, execution_context, estimated_minutes, progressive_total, scheduled_for_date, ts.to_string()],
        )?;
        Self::emit_on(
            &tx,
            identity,
            crate::crdt::CrdtTable::Directives,
            &d.id,
            &Self::directive_json(&d),
            ts,
        )?;
        for (i, (pt, pi, pm)) in phases.iter().enumerate() {
            let ts = self.hlc.now(self.device);
            tx.execute(
                "INSERT INTO directive_phases (directive_id, step, title, instruction, minutes, state, hlc_timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, (i + 1) as i64, pt, pi, pm, if i == 0 { "active" } else { "pending" }, ts.to_string()],
            )?;
            let p = DirectivePhase {
                directive_id: id.clone(),
                step: (i + 1) as i64,
                title: pt.clone(),
                instruction: pi.clone(),
                minutes: *pm,
                state: if i == 0 {
                    PhaseState::Active
                } else {
                    PhaseState::Pending
                },
                hlc_timestamp: ts,
            };
            Self::emit_on(
                &tx,
                identity,
                crate::crdt::CrdtTable::DirectivePhases,
                &id,
                &Self::phase_json(&p),
                ts,
            )?;
        }
        self.persist_head_on(&tx)?;
        tx.commit()?;
        Ok(d)
    }

    pub fn directive(&self, id: &str) -> Result<Option<Directive>, StoreError> {
        Self::directive_on(&self.conn.lock().unwrap(), id)
    }

    fn directive_on(conn: &Connection, id: &str) -> Result<Option<Directive>, StoreError> {
        conn
            .query_row(
                "SELECT id, milestone_id, title, execution_context, estimated_minutes,
                        progressive_step, progressive_total, state, scheduled_for_date, hlc_timestamp
                 FROM directives WHERE id = ?1",
                [id],
                directive_row,
            )
            .optional()
            .map_err(StoreError::Sqlite)
    }

    /// The single active directive (Stackelberg invariant: ≤1 active).
    /// Ordered to match the `enforce_single_active` winner (newest HLC,
    /// lowest id) so a transiently violated invariant still renders
    /// deterministically instead of an arbitrary row.
    pub fn active_directive(&self) -> Result<Option<Directive>, StoreError> {
        self.conn.lock().unwrap()
            .query_row(
                "SELECT id, milestone_id, title, execution_context, estimated_minutes,
                        progressive_step, progressive_total, state, scheduled_for_date, hlc_timestamp
                 FROM directives WHERE state = 'active'
                 ORDER BY hlc_timestamp DESC, id ASC LIMIT 1",
                [],
                directive_row,
            )
            .optional()
            .map_err(StoreError::Sqlite)
    }

    /// Queued directives scheduled on or before `date` (execution pool
    /// for today — NOT exposed as a list in the UI; engine only).
    pub fn runnable_directives(&self, date: &str) -> Result<Vec<Directive>, StoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, milestone_id, title, execution_context, estimated_minutes,
                    progressive_step, progressive_total, state, scheduled_for_date, hlc_timestamp
             FROM directives
             WHERE state = 'queued' AND scheduled_for_date <= ?1
             ORDER BY scheduled_for_date, hlc_timestamp, id",
        )?;
        let rows = stmt
            .query_map([date], directive_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn next_runnable_directive(&self, date: &str) -> Result<Option<Directive>, StoreError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT id, milestone_id, title, execution_context, estimated_minutes,
                    progressive_step, progressive_total, state, scheduled_for_date, hlc_timestamp
             FROM directives WHERE state = 'queued' AND scheduled_for_date <= ?1
             ORDER BY scheduled_for_date, hlc_timestamp, id LIMIT 1",
                [date],
                directive_row,
            )
            .optional()
            .map_err(StoreError::Sqlite)
    }

    pub fn set_directive_state(
        &self,
        id: &str,
        state: DirectiveState,
        identity: Option<&Identity>,
    ) -> Result<(), StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM directives WHERE id = ?1)",
            [id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(StoreError::NotFound(format!("directive {id}")));
        }
        if state == DirectiveState::Active {
            loop {
                let other: Option<String> = tx
                    .query_row(
                        "SELECT id FROM directives WHERE state = 'active' AND id != ?1 LIMIT 1",
                        [id],
                        |r| r.get(0),
                    )
                    .optional()?;
                let Some(other) = other else { break };
                Self::write_directive_state(
                    &tx,
                    &other,
                    DirectiveState::Queued,
                    self.hlc.now(self.device),
                    identity,
                )?;
            }
        }
        Self::write_directive_state(&tx, id, state, self.hlc.now(self.device), identity)?;
        let (wall, ctr) = self.hlc.head();
        tx.execute(
            "INSERT INTO hlc_clock (id, last_wall_nanos, counter, device) VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET last_wall_nanos=?1, counter=?2, device=?3",
            params![wall as i64, ctr as i64, self.device as i64],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn write_directive_state(
        conn: &Connection,
        id: &str,
        state: DirectiveState,
        ts: HlcTimestamp,
        identity: Option<&Identity>,
    ) -> Result<(), StoreError> {
        conn.execute(
            "UPDATE directives SET state = ?2, hlc_timestamp = ?3 WHERE id = ?1",
            params![id, state.as_str(), ts.to_string()],
        )?;
        if let Some(identity) = identity {
            let d = conn.query_row(
                "SELECT id, milestone_id, title, execution_context, estimated_minutes,
                        progressive_step, progressive_total, state, scheduled_for_date, hlc_timestamp
                 FROM directives WHERE id = ?1",
                [id],
                directive_row,
            )?;
            Self::emit_on(
                conn,
                Some(identity),
                crate::crdt::CrdtTable::Directives,
                id,
                &Self::directive_json(&d),
                ts,
            )?;
        }
        Ok(())
    }

    /// Reconciles the single-active invariant after a pull-apply batch:
    /// LWW arbitration can merge `active` states from two devices. The
    /// winner is deterministic across replicas (max `hlc_timestamp`,
    /// tie-break min `id`); losers are parked to queued with
    /// write-through. Returns the number parked.
    pub fn enforce_single_active(&self, identity: Option<&Identity>) -> Result<usize, StoreError> {
        let conn = self.conn.lock().unwrap();
        let tx = conn.unchecked_transaction()?;
        let actives: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM directives WHERE state = 'active'
                 ORDER BY hlc_timestamp DESC, id ASC",
            )?;
            let rows = stmt
                .query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            rows
        };
        let mut parked = 0;
        for id in actives.iter().skip(1) {
            Self::write_directive_state(
                &tx,
                id,
                DirectiveState::Queued,
                self.hlc.now(self.device),
                identity,
            )?;
            parked += 1;
        }
        let (wall, ctr) = self.hlc.head();
        tx.execute(
            "INSERT INTO hlc_clock (id, last_wall_nanos, counter, device) VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET last_wall_nanos=?1, counter=?2, device=?3",
            params![wall as i64, ctr as i64, self.device as i64],
        )?;
        tx.commit()?;
        Ok(parked)
    }

    /// Requeue a directive with a new estimate and date (scope
    /// downsizing path). Resets progressive progress to phase 1 and
    /// rescales phase minutes to the new total (E3): leaving step=2 with
    /// 5+25 min rows under a 15-min total contradicted itself forever.
    /// The directive, phase resets, outbox, and clock head commit together.
    pub fn reschedule_directive(
        &self,
        id: &str,
        estimated_minutes: i64,
        scheduled_for_date: &str,
        identity: Option<&Identity>,
    ) -> Result<(), StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut d = Self::directive_on(&tx, id)?
            .ok_or_else(|| StoreError::NotFound(format!("directive {id}")))?;
        let mut phases = Self::phases_on(&tx, id)?;
        check_minutes("new estimate", estimated_minutes).map_err(StoreError::Invalid)?;
        check_date("rescheduled date", scheduled_for_date).map_err(StoreError::Invalid)?;
        if estimated_minutes < phases.len() as i64 {
            return Err(StoreError::Invalid(
                "estimate must allow one minute per phase".into(),
            ));
        }
        if phases.iter().any(|p| p.minutes <= 0) {
            return Err(StoreError::Invalid("phase minutes must be positive".into()));
        }
        let ts = self.hlc.now(self.device);
        tx.execute(
            "UPDATE directives SET state='queued', estimated_minutes=?2, scheduled_for_date=?3,
                progressive_step=1, hlc_timestamp=?4 WHERE id=?1",
            params![id, estimated_minutes, scheduled_for_date, ts.to_string()],
        )?;
        d.state = DirectiveState::Queued;
        d.estimated_minutes = estimated_minutes;
        d.scheduled_for_date = scheduled_for_date.into();
        d.progressive_step = 1;
        d.hlc_timestamp = ts;
        Self::emit_on(
            &tx,
            identity,
            crate::crdt::CrdtTable::Directives,
            id,
            &Self::directive_json(&d),
            ts,
        )?;
        // Rescale phase minutes proportionally so the rows sum to the
        // new total (floor 1 min per phase; the last phase absorbs
        // rounding drift). Monolithic directives have no rows: no-op.
        if !phases.is_empty() {
            let old_total: i128 = phases.iter().map(|p| i128::from(p.minutes)).sum();
            let mut remaining = estimated_minutes;
            let phase_count = phases.len();
            for (i, p) in phases.iter_mut().enumerate() {
                let reserved = (phase_count - i - 1) as i64;
                let scaled = if reserved == 0 {
                    remaining
                } else {
                    let rounded = (i128::from(p.minutes) * i128::from(estimated_minutes)
                        + old_total / 2)
                        / old_total;
                    (rounded as i64).clamp(1, remaining - reserved)
                };
                remaining -= scaled;
                let ts = self.hlc.now(self.device);
                let state = if p.step == 1 { "active" } else { "pending" };
                tx.execute(
                    "UPDATE directive_phases SET minutes=?3, state=?4, hlc_timestamp=?5
                     WHERE directive_id=?1 AND step=?2",
                    params![id, p.step, scaled, state, ts.to_string()],
                )?;
                p.minutes = scaled;
                p.state = if p.step == 1 {
                    PhaseState::Active
                } else {
                    PhaseState::Pending
                };
                p.hlc_timestamp = ts;
                Self::emit_on(
                    &tx,
                    identity,
                    crate::crdt::CrdtTable::DirectivePhases,
                    id,
                    &Self::phase_json(p),
                    ts,
                )?;
            }
        }
        self.persist_head_on(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn advance_progressive_step(
        &self,
        id: &str,
        identity: Option<&Identity>,
    ) -> Result<bool, StoreError> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut d = Self::directive_on(&tx, id)?
            .ok_or_else(|| StoreError::NotFound(format!("directive {id}")))?;
        let mut phases = Self::phases_on(&tx, id)?;
        let cur = d.progressive_step;
        let advanced = cur < d.progressive_total;
        if !phases.iter().any(|p| p.step == cur)
            || (advanced && !phases.iter().any(|p| p.step == cur + 1))
        {
            return Err(StoreError::Invalid("current or next phase missing".into()));
        }
        for p in phases
            .iter_mut()
            .filter(|p| p.step == cur || (advanced && p.step == cur + 1))
        {
            p.state = if p.step == cur {
                PhaseState::Done
            } else {
                PhaseState::Active
            };
            p.hlc_timestamp = self.hlc.now(self.device);
            tx.execute(
                "UPDATE directive_phases SET state = ?3, hlc_timestamp = ?4
                 WHERE directive_id = ?1 AND step = ?2",
                params![id, p.step, p.state.as_str(), p.hlc_timestamp.to_string()],
            )?;
            Self::emit_on(
                &tx,
                identity,
                crate::crdt::CrdtTable::DirectivePhases,
                id,
                &Self::phase_json(p),
                p.hlc_timestamp,
            )?;
        }
        if advanced {
            d.progressive_step = cur + 1;
            d.hlc_timestamp = self.hlc.now(self.device);
            tx.execute(
                "UPDATE directives SET progressive_step = ?2, hlc_timestamp = ?3 WHERE id = ?1",
                params![id, d.progressive_step, d.hlc_timestamp.to_string()],
            )?;
            Self::emit_on(
                &tx,
                identity,
                crate::crdt::CrdtTable::Directives,
                id,
                &Self::directive_json(&d),
                d.hlc_timestamp,
            )?;
        }
        self.persist_head_on(&tx)?;
        tx.commit()?;
        Ok(advanced)
    }

    pub fn phases_for_directive(
        &self,
        directive_id: &str,
    ) -> Result<Vec<DirectivePhase>, StoreError> {
        Self::phases_on(&self.conn.lock().unwrap(), directive_id)
    }

    fn phases_on(conn: &Connection, directive_id: &str) -> Result<Vec<DirectivePhase>, StoreError> {
        let mut stmt = conn.prepare(
            "SELECT directive_id, step, title, instruction, minutes, state, hlc_timestamp
             FROM directive_phases WHERE directive_id = ?1 ORDER BY step",
        )?;
        let rows = stmt
            .query_map([directive_id], |r| {
                Ok(DirectivePhase {
                    directive_id: r.get(0)?,
                    step: r.get(1)?,
                    title: r.get(2)?,
                    instruction: r.get(3)?,
                    minutes: r.get(4)?,
                    state: parse_enum(
                        "phase state",
                        &r.get::<_, String>(5)?,
                        PhaseState::from_str,
                    )?,
                    hlc_timestamp: parse_hlc(&r.get::<_, String>(6)?)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Count of completed directives for a milestone (velocity data).
    pub fn completed_count_for_milestone(&self, milestone_id: &str) -> Result<i64, StoreError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM directives WHERE milestone_id = ?1 AND state = 'completed'",
                [milestone_id],
                |r| r.get(0),
            )
            .map_err(StoreError::Sqlite)
    }

    // ------------------------------------------------------------------
    // Check-ins
    // ------------------------------------------------------------------

    pub fn upsert_check_in(
        &self,
        date: &str,
        outcome: CheckInOutcome,
        note: Option<&str>,
        identity: Option<&Identity>,
    ) -> Result<CheckIn, StoreError> {
        check_date("check-in date", date).map_err(StoreError::Invalid)?;
        check_optional_text("check-in note", note, MAX_NOTE_CHARS).map_err(StoreError::Invalid)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let existing: Option<String> = tx
            .query_row("SELECT id FROM check_ins WHERE date = ?1", [date], |r| {
                r.get(0)
            })
            .optional()?;
        let ts = self.hlc.now(self.device);
        let id = existing.unwrap_or_else(|| new_id("chk"));
        tx.execute(
            "INSERT INTO check_ins (id, date, outcome, note, hlc_timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(date) DO UPDATE SET outcome=excluded.outcome,
                note=excluded.note, hlc_timestamp=excluded.hlc_timestamp",
            params![id, date, outcome.as_str(), note, ts.to_string()],
        )?;
        let c = CheckIn {
            id,
            date: date.into(),
            outcome,
            note: note.map(Into::into),
            hlc_timestamp: ts,
        };
        Self::emit_on(
            &tx,
            identity,
            crate::crdt::CrdtTable::CheckIns,
            &c.id,
            &Self::checkin_json(&c),
            ts,
        )?;
        self.persist_head_on(&tx)?;
        tx.commit()?;
        Ok(c)
    }

    pub fn check_in_for_date(&self, date: &str) -> Result<Option<CheckIn>, StoreError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT id, date, outcome, note, hlc_timestamp FROM check_ins WHERE date = ?1",
                [date],
                |r| {
                    Ok(CheckIn {
                        id: r.get(0)?,
                        date: r.get(1)?,
                        outcome: parse_enum(
                            "check-in outcome",
                            &r.get::<_, String>(2)?,
                            CheckInOutcome::from_str,
                        )?,
                        note: r.get(3)?,
                        hlc_timestamp: parse_hlc(&r.get::<_, String>(4)?)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::Sqlite)
    }

    /// Recent check-in outcomes ordered by date desc (velocity context
    /// for Tier 2 / recalibration).
    pub fn recent_check_ins(&self, days: i64) -> Result<Vec<CheckIn>, StoreError> {
        if !(0..=1024).contains(&days) {
            return Err(StoreError::Invalid(
                "check-in limit must be in 0–1024".into(),
            ));
        }
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, date, outcome, note, hlc_timestamp FROM check_ins
             ORDER BY date DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([days], |r| {
                Ok(CheckIn {
                    id: r.get(0)?,
                    date: r.get(1)?,
                    outcome: parse_enum(
                        "check-in outcome",
                        &r.get::<_, String>(2)?,
                        CheckInOutcome::from_str,
                    )?,
                    note: r.get(3)?,
                    hlc_timestamp: parse_hlc(&r.get::<_, String>(4)?)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ------------------------------------------------------------------
    // Bailouts
    // ------------------------------------------------------------------

    pub fn record_bailout(
        &self,
        directive_id: &str,
        reason: BailoutReason,
        note: Option<&str>,
        identity: Option<&Identity>,
    ) -> Result<Bailout, StoreError> {
        // Spec cap (escape-hatch modal): truncate, never reject — the UI
        // already trims to 140 chars; this keeps other producers honest.
        let note: Option<String> = note.map(|n| n.chars().take(MAX_BAILOUT_NOTE_CHARS).collect());
        let id = new_id("bail");
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let ts = self.hlc.now(self.device);
        let b = Bailout {
            id: id.clone(),
            directive_id: directive_id.to_string(),
            reason,
            note: note.clone(),
            hlc_timestamp: ts,
        };
        tx.execute(
            "INSERT INTO bailouts (id, directive_id, reason, note, hlc_timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, directive_id, reason.as_str(), note, ts.to_string()],
        )?;
        Self::emit_on(
            &tx,
            identity,
            crate::crdt::CrdtTable::Bailouts,
            &b.id,
            &Self::bailout_json(&b),
            ts,
        )?;
        self.persist_head_on(&tx)?;
        tx.commit()?;
        Ok(b)
    }

    // ------------------------------------------------------------------
    // Settings
    // ------------------------------------------------------------------

    pub fn settings(&self) -> Result<AppSettings, StoreError> {
        self.conn.lock().unwrap()
            .query_row(
                "SELECT theme, hotkey, always_on_top, ai_provider, tier1_model, tier2_model, relay_url
                 FROM app_settings WHERE id = 1",
                [],
                |r| {
                    Ok(AppSettings {
                        theme: r.get(0)?,
                        hotkey: r.get(1)?,
                        always_on_top: r.get::<_, i64>(2)? != 0,
                        ai_provider: r.get(3)?,
                        tier1_model: r.get(4)?,
                        tier2_model: r.get(5)?,
                        relay_url: r.get(6)?,
                    })
                },
            )
            .optional()
            .map(|o| o.unwrap_or_default())
            .map_err(StoreError::Sqlite)
    }

    pub fn save_settings(
        &self,
        s: &AppSettings,
        identity: Option<&Identity>,
    ) -> Result<(), StoreError> {
        if s.theme != "dark" && s.theme != "light" {
            return Err(StoreError::Invalid(
                "theme must be 'dark' or 'light'".into(),
            ));
        }
        // Empty hotkey normalizes to the default (boot does the same);
        // anything longer than a plausible accelerator is junk.
        let hotkey = if s.hotkey.trim().is_empty() {
            "alt+space".to_string()
        } else {
            s.hotkey.clone()
        };
        if hotkey.chars().count() > 64 {
            return Err(StoreError::Invalid("hotkey is too long".into()));
        }
        if let Some(p) = s.ai_provider.as_deref() {
            if !KNOWN_PROVIDERS.contains(&p) {
                return Err(StoreError::Invalid("unknown AI provider".into()));
            }
        }
        for (field, v) in [
            ("tier1 model", s.tier1_model.as_deref()),
            ("tier2 model", s.tier2_model.as_deref()),
        ] {
            check_optional_text(field, v, MAX_MODEL_ID_CHARS).map_err(StoreError::Invalid)?;
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let ts = self.hlc.now(self.device);
        // Row and op payload carry the same normalized hotkey so a pull
        // can never resurrect the pre-normalization value.
        let normalized = AppSettings {
            hotkey,
            ..s.clone()
        };
        tx.execute(
            "INSERT INTO app_settings (id, theme, hotkey, always_on_top, ai_provider, tier1_model, tier2_model, relay_url, hlc_timestamp)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET theme=?1, hotkey=?2, always_on_top=?3,
                ai_provider=?4, tier1_model=?5, tier2_model=?6, relay_url=?7, hlc_timestamp=?8",
            params![normalized.theme, normalized.hotkey, normalized.always_on_top as i64, normalized.ai_provider, normalized.tier1_model, normalized.tier2_model, normalized.relay_url, ts.to_string()],
        )?;
        Self::emit_on(
            &tx,
            identity,
            crate::crdt::CrdtTable::AppSettings,
            "settings",
            &Self::settings_json(&normalized),
            ts,
        )?;
        self.persist_head_on(&tx)?;
        tx.commit()?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // CRDT outbox (PRD §7 `crdt_outbox`) — write-through from domain ops.
    // ------------------------------------------------------------------

    /// Canonical row JSON for an op payload. Keys MUST match what
    /// `wl_sync::sync::apply_op_to_db` reads on the pull side; the
    /// round-trip is covered by `write_through_syncs_all_tables`.
    fn goal_json(g: &Goal) -> serde_json::Value {
        serde_json::json!({
            "title": g.title,
            "description": g.description,
            "target_date": g.target_date,
            "status": g.status.as_str(),
        })
    }

    fn milestone_json(m: &Milestone) -> serde_json::Value {
        serde_json::json!({
            "goal_id": m.goal_id,
            "title": m.title,
            "description": m.description,
            "order_index": m.order_index,
            "status": m.status.as_str(),
        })
    }

    fn directive_json(d: &Directive) -> serde_json::Value {
        serde_json::json!({
            "milestone_id": d.milestone_id,
            "title": d.title,
            "execution_context": d.execution_context,
            "estimated_minutes": d.estimated_minutes,
            "progressive_step": d.progressive_step,
            "progressive_total": d.progressive_total,
            "state": d.state.as_str(),
            "scheduled_for_date": d.scheduled_for_date,
        })
    }

    fn phase_json(p: &DirectivePhase) -> serde_json::Value {
        serde_json::json!({
            "step": p.step,
            "title": p.title,
            "instruction": p.instruction,
            "minutes": p.minutes,
            "state": p.state.as_str(),
        })
    }

    fn checkin_json(c: &CheckIn) -> serde_json::Value {
        serde_json::json!({
            "date": c.date,
            "outcome": c.outcome.as_str(),
            "note": c.note,
        })
    }

    fn bailout_json(b: &Bailout) -> serde_json::Value {
        serde_json::json!({
            "directive_id": b.directive_id,
            "reason": b.reason.as_str(),
            "note": b.note,
        })
    }

    fn settings_json(s: &AppSettings) -> serde_json::Value {
        serde_json::json!({
            "theme": s.theme,
            "hotkey": s.hotkey,
            // Numeric: the pull side reads `as_i64`.
            "always_on_top": if s.always_on_top { 1 } else { 0 },
            "ai_provider": s.ai_provider,
            "tier1_model": s.tier1_model,
            "tier2_model": s.tier2_model,
            "relay_url": s.relay_url,
        })
    }

    /// Enqueues the exact row version within the caller's transaction.
    fn emit_on(
        conn: &Connection,
        identity: Option<&Identity>,
        table: crate::crdt::CrdtTable,
        record_id: &str,
        payload: &serde_json::Value,
        ts: HlcTimestamp,
    ) -> Result<(), StoreError> {
        if let Some(identity) = identity {
            Self::enqueue_on(conn, identity, table.as_str(), record_id, payload, ts)?;
        }
        Ok(())
    }

    fn enqueue_on(
        conn: &Connection,
        identity: &Identity,
        table_name: &str,
        record_id: &str,
        payload_json: &serde_json::Value,
        ts: HlcTimestamp,
    ) -> Result<String, StoreError> {
        let op_id = new_id("op");
        let aad = format!("{table_name}:{record_id}");
        let plaintext = zeroize::Zeroizing::new(
            serde_json::to_vec(payload_json).map_err(|e| StoreError::Invalid(e.to_string()))?,
        );
        let sealed = aead::seal(identity, &plaintext, aad.as_bytes())
            .map_err(|e| StoreError::Invalid(e.to_string()))?;
        conn.execute(
            "INSERT INTO crdt_outbox (operation_id, hlc_timestamp, table_name, record_id, encrypted_payload, created_at_epoch_ms, pushed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
            params![op_id, ts.to_string(), table_name, record_id, sealed.to_bytes(), ts.epoch_ms() as i64],
        )?;
        Ok(op_id)
    }

    fn persist_head_on(&self, conn: &Connection) -> Result<(), StoreError> {
        let (wall, ctr) = self.hlc.head();
        conn.execute(
            "INSERT INTO hlc_clock (id, last_wall_nanos, counter, device) VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET last_wall_nanos=?1, counter=?2, device=?3",
            params![wall as i64, ctr as i64, self.device as i64],
        )?;
        Ok(())
    }

    /// Enqueue an encrypted CRDT op for a row mutation. AAD binds the
    /// ciphertext to its routing header (table:record). The table must be
    /// a known syncable table — unknown tables wedge peer merges.
    pub fn enqueue_outbox(
        &self,
        identity: &Identity,
        table_name: &str,
        record_id: &str,
        payload_json: &serde_json::Value,
    ) -> Result<String, StoreError> {
        if crate::crdt::CrdtTable::from_str(table_name).is_none() {
            return Err(StoreError::Invalid("unknown CRDT table".into()));
        }
        if record_id.is_empty() || record_id.len() > 128 {
            return Err(StoreError::Invalid("bad CRDT record id".into()));
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let ts = self.hlc.now(self.device);
        let op_id = Self::enqueue_on(&tx, identity, table_name, record_id, payload_json, ts)?;
        self.persist_head_on(&tx)?;
        tx.commit()?;
        Ok(op_id)
    }

    /// Pending (unpushed) outbox operations, oldest first. The tie-break
    /// on `(hlc_timestamp, operation_id)` is load-bearing: ops enqueued
    /// within the same wall millisecond share `created_at_epoch_ms`, and
    /// a bare `ORDER BY created_at_epoch_ms` lets SQLite return either
    /// order — retries could then re-push the same ops shuffled.
    pub fn pending_outbox(&self, limit: i64) -> Result<Vec<OutboxOp>, StoreError> {
        if !(0..=1024).contains(&limit) {
            return Err(StoreError::Invalid("outbox limit must be in 0–1024".into()));
        }
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT operation_id, hlc_timestamp, table_name, record_id, encrypted_payload
             FROM crdt_outbox WHERE pushed = 0
             ORDER BY created_at_epoch_ms, hlc_timestamp, operation_id LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit], |r| {
                Ok(OutboxOp {
                    operation_id: r.get(0)?,
                    hlc_timestamp: parse_hlc(&r.get::<_, String>(1)?)?,
                    table_name: r.get(2)?,
                    record_id: r.get(3)?,
                    encrypted_payload: r.get(4)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn mark_outbox_pushed(&self, operation_ids: &[String]) -> Result<(), StoreError> {
        // Single transaction: one lock acquisition, atomic drain state —
        // a crash mid-batch can never leave half the batch pushed.
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        for id in operation_ids {
            tx.execute(
                "UPDATE crdt_outbox SET pushed = 1 WHERE operation_id = ?1",
                [id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Deletes relay-durable outbox rows. Called after the push phase:
    /// every `pushed = 1` op was acknowledged (accepted or duplicate) by
    /// the relay, so it is never read locally again — without this the
    /// outbox grows forever on long-lived installs.
    pub fn delete_pushed_outbox(&self) -> Result<usize, StoreError> {
        let n = self
            .conn
            .lock()
            .unwrap()
            .execute("DELETE FROM crdt_outbox WHERE pushed = 1", [])?;
        Ok(n)
    }

    // ------------------------------------------------------------------
    // Applied-op watermark (idempotent remote apply).
    // ------------------------------------------------------------------

    pub fn is_op_applied(&self, operation_id: &str) -> Result<bool, StoreError> {
        self.conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM crdt_applied WHERE operation_id = ?1",
                [operation_id],
                |r| r.get::<_, i64>(0),
            )
            .map(|c| c > 0)
            .map_err(StoreError::Sqlite)
    }

    pub fn mark_op_applied(&self, operation_id: &str, ts: HlcTimestamp) -> Result<(), StoreError> {
        self.mark_op_applied_str(operation_id, &ts.to_string())
    }

    /// Watermark insert with a pre-rendered HLC string — the quarantine
    /// path for undecryptable/unparseable remote ops, which have no
    /// usable timestamp but must still advance past the poison op.
    pub fn mark_op_applied_str(&self, operation_id: &str, hlc: &str) -> Result<(), StoreError> {
        self.conn.lock().unwrap().execute(
            "INSERT OR IGNORE INTO crdt_applied (operation_id, hlc_timestamp) VALUES (?1, ?2)",
            params![operation_id, hlc],
        )?;
        Ok(())
    }

    /// Drops applied watermarks at or below the persisted pull cursor.
    /// The relay paginates strictly after the cursor, so those ops can
    /// never be re-pulled — without this `crdt_applied` grows forever.
    /// Callers skip the prune while the cursor is still ("", "").
    pub fn prune_applied_below(&self, hlc: &str, op_id: &str) -> Result<usize, StoreError> {
        let n = self.conn.lock().unwrap().execute(
            "DELETE FROM crdt_applied
              WHERE hlc_timestamp < ?1 OR (hlc_timestamp = ?1 AND operation_id <= ?2)",
            params![hlc, op_id],
        )?;
        Ok(n)
    }

    /// This replica's device id (for HLC receive-event merges).
    pub fn device_id(&self) -> u16 {
        self.device
    }

    /// Merges a pulled remote timestamp into the local HLC clock and
    /// persists the head (receive event). Without this, a pull from a
    /// fast peer followed by a local tick can issue `ts < remote-ts`
    /// and LWW arbitration inverts (causality gap).
    pub fn observe_remote_hlc(&self, remote: &HlcTimestamp) -> HlcTimestamp {
        let ts = self.hlc.observe(remote, self.device);
        let _ = self.write_hlc_head();
        ts
    }
}

/// A pending outbox operation (decrypted locally; ciphertext only over
/// the wire).
#[derive(Debug, Clone)]
pub struct OutboxOp {
    pub operation_id: String,
    pub hlc_timestamp: HlcTimestamp,
    pub table_name: String,
    pub record_id: String,
    pub encrypted_payload: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Row mappers
// ---------------------------------------------------------------------------

fn parse_hlc(s: &str) -> Result<HlcTimestamp, rusqlite::Error> {
    HlcTimestamp::parse(s).map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))
}

/// A corrupt enum string in a row. Returned as a SQLite-mapping error
/// (surfaces as `StoreError::Sqlite`) — never a panic: a single bad
/// row must not abort the shell, and remote LWW upserts can only write
/// strings the local writer produced.
#[derive(Debug)]
struct BadEnum {
    column: &'static str,
    value: String,
}

impl std::fmt::Display for BadEnum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid {} value {:?}", self.column, self.value)
    }
}

impl std::error::Error for BadEnum {}

fn parse_enum<T>(
    column: &'static str,
    value: &str,
    f: impl FnOnce(&str) -> Option<T>,
) -> rusqlite::Result<T> {
    f(value).ok_or_else(|| {
        rusqlite::Error::ToSqlConversionFailure(Box::new(BadEnum {
            column,
            value: value.to_string(),
        }))
    })
}

fn goal_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Goal> {
    Ok(Goal {
        id: r.get(0)?,
        title: r.get(1)?,
        description: r.get(2)?,
        target_date: r.get(3)?,
        status: parse_enum("goal status", &r.get::<_, String>(4)?, GoalStatus::from_str)?,
        hlc_timestamp: parse_hlc(&r.get::<_, String>(5)?)?,
    })
}

fn milestone_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Milestone> {
    Ok(Milestone {
        id: r.get(0)?,
        goal_id: r.get(1)?,
        title: r.get(2)?,
        description: r.get(3)?,
        order_index: r.get(4)?,
        status: parse_enum(
            "milestone status",
            &r.get::<_, String>(5)?,
            MilestoneStatus::from_str,
        )?,
        hlc_timestamp: parse_hlc(&r.get::<_, String>(6)?)?,
    })
}

fn directive_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Directive> {
    Ok(Directive {
        id: r.get(0)?,
        milestone_id: r.get(1)?,
        title: r.get(2)?,
        execution_context: r.get(3)?,
        estimated_minutes: r.get(4)?,
        progressive_step: r.get(5)?,
        progressive_total: r.get(6)?,
        state: parse_enum(
            "directive state",
            &r.get::<_, String>(7)?,
            DirectiveState::from_str,
        )?,
        scheduled_for_date: r.get(8)?,
        hlc_timestamp: parse_hlc(&r.get::<_, String>(9)?)?,
    })
}
