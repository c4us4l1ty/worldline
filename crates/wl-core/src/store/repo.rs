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
        let (wall, ctr) = self.hlc.head();
        self.conn.lock().unwrap().execute(
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
        let id = new_id("goal");
        let ts = self.tick();
        let goal = Goal {
            id: id.clone(),
            title: title.to_string(),
            description: description.map(Into::into),
            target_date: target_date.map(Into::into),
            status: GoalStatus::Active,
            hlc_timestamp: ts,
        };
        self.conn.lock().unwrap().execute(
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
        self.emit(
            identity,
            crate::crdt::CrdtTable::Goals,
            &goal.id,
            &Self::goal_json(&goal),
        )?;
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
        let id = new_id("ms");
        let ts = self.tick();
        let m = Milestone {
            id: id.clone(),
            goal_id: goal_id.to_string(),
            title: title.to_string(),
            description: description.map(Into::into),
            order_index,
            status: MilestoneStatus::Pending,
            hlc_timestamp: ts,
        };
        self.conn.lock().unwrap().execute(
            "INSERT INTO milestones (id, goal_id, title, description, order_index, status, hlc_timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![id, goal_id, title, description, order_index, "pending", ts.to_string()],
        )?;
        self.emit(
            identity,
            crate::crdt::CrdtTable::Milestones,
            &m.id,
            &Self::milestone_json(&m),
        )?;
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
        let ts = self.tick();
        self.conn.lock().unwrap().execute(
            "UPDATE milestones SET status = ?2, hlc_timestamp = ?3 WHERE id = ?1",
            params![id, status.as_str(), ts.to_string()],
        )?;
        if identity.is_some() {
            if let Some(m) = self.milestone(id)? {
                self.emit(
                    identity,
                    crate::crdt::CrdtTable::Milestones,
                    &m.id,
                    &Self::milestone_json(&m),
                )?;
            }
        }
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
        if progressive_total > 1 && phases.len() as i64 != progressive_total {
            return Err(StoreError::Invalid(format!(
                "progressive_total={progressive_total} but {} phases supplied",
                phases.len()
            )));
        }
        let id = new_id("dir");
        let ts = self.tick();
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
        self.conn.lock().unwrap().execute(
            "INSERT INTO directives (id, milestone_id, title, execution_context,
                estimated_minutes, progressive_step, progressive_total, state, scheduled_for_date, hlc_timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, 'queued', ?7, ?8)",
            params![id, milestone_id, title, execution_context, estimated_minutes, progressive_total, scheduled_for_date, ts.to_string()],
        )?;
        for (i, (pt, pi, pm)) in phases.iter().enumerate() {
            let ts = self.tick();
            self.conn.lock().unwrap().execute(
                "INSERT INTO directive_phases (directive_id, step, title, instruction, minutes, state, hlc_timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, (i + 1) as i64, pt, pi, pm, if i == 0 { "active" } else { "pending" }, ts.to_string()],
            )?;
        }
        if identity.is_some() {
            self.emit(
                identity,
                crate::crdt::CrdtTable::Directives,
                &d.id,
                &Self::directive_json(&d),
            )?;
            for p in self.phases_for_directive(&d.id)? {
                self.emit(
                    identity,
                    crate::crdt::CrdtTable::DirectivePhases,
                    &d.id,
                    &Self::phase_json(&p),
                )?;
            }
        }
        Ok(d)
    }

    pub fn directive(&self, id: &str) -> Result<Option<Directive>, StoreError> {
        self.conn.lock().unwrap()
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
    pub fn active_directive(&self) -> Result<Option<Directive>, StoreError> {
        self.conn.lock().unwrap()
            .query_row(
                "SELECT id, milestone_id, title, execution_context, estimated_minutes,
                        progressive_step, progressive_total, state, scheduled_for_date, hlc_timestamp
                 FROM directives WHERE state = 'active' LIMIT 1",
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
             ORDER BY scheduled_for_date, hlc_timestamp",
        )?;
        let rows = stmt
            .query_map([date], directive_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_directive_state(
        &self,
        id: &str,
        state: DirectiveState,
        identity: Option<&Identity>,
    ) -> Result<(), StoreError> {
        // Stackelberg invariant: at most one directive is ever active.
        // Park any *other* active back to queued first (self-healing for
        // stray callers and merged remote states — never hide a second
        // active behind `LIMIT 1`). Each parked row gets its own tick +
        // outbox write-through so peers converge on the same single
        // active.
        if state == DirectiveState::Active {
            let others: Vec<String> = {
                let conn = self.conn.lock().unwrap();
                let mut stmt = conn.prepare(
                    "SELECT id FROM directives WHERE state = 'active' AND id != ?1",
                )?;
                stmt.query_map([id], |r| r.get(0))?
                    .collect::<Result<_, _>>()?
            };
            for other in others {
                let pts = self.tick();
                self.conn.lock().unwrap().execute(
                    "UPDATE directives SET state = 'queued', hlc_timestamp = ?2 WHERE id = ?1",
                    params![other, pts.to_string()],
                )?;
                if identity.is_some() {
                    if let Some(d) = self.directive(&other)? {
                        self.emit(
                            identity,
                            crate::crdt::CrdtTable::Directives,
                            &d.id,
                            &Self::directive_json(&d),
                        )?;
                    }
                }
            }
        }
        let ts = self.tick();
        self.conn.lock().unwrap().execute(
            "UPDATE directives SET state = ?2, hlc_timestamp = ?3 WHERE id = ?1",
            params![id, state.as_str(), ts.to_string()],
        )?;
        if identity.is_some() {
            if let Some(d) = self.directive(id)? {
                self.emit(
                    identity,
                    crate::crdt::CrdtTable::Directives,
                    &d.id,
                    &Self::directive_json(&d),
                )?;
            }
        }
        Ok(())
    }

    /// Reconciles the single-active invariant after a pull-apply batch:
    /// LWW arbitration can merge `active` states from two devices. The
    /// winner is deterministic across replicas (max `hlc_timestamp`,
    /// tie-break min `id`); losers are parked to queued with
    /// write-through. Returns the number parked.
    pub fn enforce_single_active(
        &self,
        identity: Option<&Identity>,
    ) -> Result<usize, StoreError> {
        let actives: Vec<String> = {
            let conn = self.conn.lock().unwrap();
            let mut stmt = conn.prepare(
                "SELECT id FROM directives WHERE state = 'active'
                 ORDER BY hlc_timestamp DESC, id ASC",
            )?;
            stmt.query_map([], |r| r.get(0))?
                .collect::<Result<_, _>>()?
        };
        let mut parked = 0;
        for id in actives.iter().skip(1) {
            self.set_directive_state(id, DirectiveState::Queued, identity)?;
            parked += 1;
        }
        Ok(parked)
    }

    /// Requeue a directive with a new estimate and date (scope
    /// downsizing path). Resets progressive progress to phase 1 and
    /// rescales phase minutes to the new total (E3): leaving step=2 with
    /// 5+25 min rows under a 15-min total contradicted itself forever.
    /// Single statement for the directive row + phase reset, then
    /// write-through of everything touched.
    pub fn reschedule_directive(
        &self,
        id: &str,
        estimated_minutes: i64,
        scheduled_for_date: &str,
        identity: Option<&Identity>,
    ) -> Result<(), StoreError> {
        let ts = self.tick();
        self.conn.lock().unwrap().execute(
            "UPDATE directives SET state='queued', estimated_minutes=?2, scheduled_for_date=?3,
                progressive_step=1, hlc_timestamp=?4 WHERE id=?1",
            params![id, estimated_minutes, scheduled_for_date, ts.to_string()],
        )?;
        // Rescale phase minutes proportionally so the rows sum to the
        // new total (floor 1 min per phase; the last phase absorbs
        // rounding drift). Monolithic directives have no rows: no-op.
        let phases = self.phases_for_directive(id)?;
        if !phases.is_empty() {
            let old_total: i64 = phases.iter().map(|p| p.minutes).sum();
            if old_total > 0 {
                let mut acc = 0i64;
                for (i, p) in phases.iter().enumerate() {
                    let scaled = if i + 1 == phases.len() {
                        (estimated_minutes - acc).max(1)
                    } else {
                        ((p.minutes as f64 * estimated_minutes as f64 / old_total as f64).round()
                            as i64)
                            .max(1)
                    };
                    acc += scaled;
                    let ts = self.tick();
                    let state = if p.step == 1 { "active" } else { "pending" };
                    self.conn.lock().unwrap().execute(
                        "UPDATE directive_phases SET minutes=?3, state=?4, hlc_timestamp=?5
                         WHERE directive_id=?1 AND step=?2",
                        params![id, p.step, scaled, state, ts.to_string()],
                    )?;
                }
            } else {
                let ts = self.tick();
                self.conn.lock().unwrap().execute(
                    "UPDATE directive_phases SET state=(CASE WHEN step=1 THEN 'active' ELSE 'pending' END),
                        hlc_timestamp=?2 WHERE directive_id=?1",
                    params![id, ts.to_string()],
                )?;
            }
        }
        if identity.is_some() {
            if let Some(d) = self.directive(id)? {
                self.emit(
                    identity,
                    crate::crdt::CrdtTable::Directives,
                    &d.id,
                    &Self::directive_json(&d),
                )?;
            }
            for p in self.phases_for_directive(id)? {
                self.emit(
                    identity,
                    crate::crdt::CrdtTable::DirectivePhases,
                    id,
                    &Self::phase_json(&p),
                )?;
            }
        }
        Ok(())
    }

    pub fn advance_progressive_step(
        &self,
        id: &str,
        identity: Option<&Identity>,
    ) -> Result<bool, StoreError> {
        // Mark current phase done, activate next; returns true when a
        // next phase exists (directive itself continues). The final
        // phase is marked done too (E2) and always replicated when an
        // identity is present — otherwise the done-state never reaches
        // peers.
        let d = self
            .directive(id)?
            .ok_or_else(|| StoreError::NotFound(format!("directive {id}")))?;
        let cur = d.progressive_step;
        let ts = self.tick();
        self.conn.lock().unwrap().execute(
            "UPDATE directive_phases SET state = 'done', hlc_timestamp = ?3 WHERE directive_id = ?1 AND step = ?2",
            params![id, cur, ts.to_string()],
        )?;
        let advanced = cur < d.progressive_total;
        if advanced {
            let ts = self.tick();
            self.conn.lock().unwrap().execute(
                "UPDATE directive_phases SET state = 'active', hlc_timestamp = ?3 WHERE directive_id = ?1 AND step = ?2",
                params![id, cur + 1, ts.to_string()],
            )?;
            self.conn.lock().unwrap().execute(
                "UPDATE directives SET progressive_step = ?2, hlc_timestamp = ?3 WHERE id = ?1",
                params![id, cur + 1, ts.to_string()],
            )?;
        }
        if identity.is_some() {
            if let Some(dd) = self.directive(id)? {
                self.emit(
                    identity,
                    crate::crdt::CrdtTable::Directives,
                    &dd.id,
                    &Self::directive_json(&dd),
                )?;
            }
            for p in self.phases_for_directive(id)? {
                self.emit(
                    identity,
                    crate::crdt::CrdtTable::DirectivePhases,
                    id,
                    &Self::phase_json(&p),
                )?;
            }
        }
        Ok(advanced)
    }

    pub fn phases_for_directive(
        &self,
        directive_id: &str,
    ) -> Result<Vec<DirectivePhase>, StoreError> {
        let conn = self.conn.lock().unwrap();
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
                    state: parse_enum("phase state", &r.get::<_, String>(5)?, PhaseState::from_str)?,
                    hlc_timestamp: parse_hlc(&r.get::<_, String>(6)?)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// LWW-guarded phase state write: only applies when `ts` is newer
    /// than the row's current hlc. Returns `true` when written.
    pub fn set_phase_state_lww(
        &self,
        directive_id: &str,
        step: i64,
        state: PhaseState,
        ts: HlcTimestamp,
    ) -> Result<bool, StoreError> {
        let changed = self.conn.lock().unwrap().execute(
            "UPDATE directive_phases SET state = ?3, hlc_timestamp = ?4
             WHERE directive_id = ?1 AND step = ?2 AND hlc_timestamp < ?4",
            params![directive_id, step, state.as_str(), ts.to_string()],
        )?;
        Ok(changed > 0)
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
        let existing: Option<String> = self
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT id FROM check_ins WHERE date = ?1", [date], |r| {
                r.get(0)
            })
            .optional()
            .map_err(StoreError::Sqlite)?;
        let ts = self.tick();
        match existing {
            Some(id) => {
                self.conn.lock().unwrap().execute(
                    "UPDATE check_ins SET outcome = ?2, note = ?3, hlc_timestamp = ?4 WHERE id = ?1",
                    params![id, outcome.as_str(), note, ts.to_string()],
                )?;
            }
            None => {
                let id = new_id("chk");
                self.conn.lock().unwrap().execute(
                    "INSERT INTO check_ins (id, date, outcome, note, hlc_timestamp)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![id, date, outcome.as_str(), note, ts.to_string()],
                )?;
            }
        }
        self.check_in_for_date(date)?
            .ok_or_else(|| StoreError::Invalid("check-in vanished after upsert".into()))
            .and_then(|c| {
                if identity.is_some() {
                    self.emit(
                        identity,
                        crate::crdt::CrdtTable::CheckIns,
                        &c.id,
                        &Self::checkin_json(&c),
                    )?;
                }
                Ok(c)
            })
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
                    outcome: CheckInOutcome::from_str(&r.get::<_, String>(2)?).expect("db outcome"),
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
        let id = new_id("bail");
        let ts = self.tick();
        let b = Bailout {
            id: id.clone(),
            directive_id: directive_id.to_string(),
            reason,
            note: note.map(Into::into),
            hlc_timestamp: ts,
        };
        self.conn.lock().unwrap().execute(
            "INSERT INTO bailouts (id, directive_id, reason, note, hlc_timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, directive_id, reason.as_str(), note, ts.to_string()],
        )?;
        self.emit(
            identity,
            crate::crdt::CrdtTable::Bailouts,
            &b.id,
            &Self::bailout_json(&b),
        )?;
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
        let ts = self.tick();
        self.conn.lock().unwrap().execute(
            "INSERT INTO app_settings (id, theme, hotkey, always_on_top, ai_provider, tier1_model, tier2_model, relay_url, hlc_timestamp)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET theme=?1, hotkey=?2, always_on_top=?3,
                ai_provider=?4, tier1_model=?5, tier2_model=?6, relay_url=?7, hlc_timestamp=?8",
            params![s.theme, s.hotkey, s.always_on_top as i64, s.ai_provider, s.tier1_model, s.tier2_model, s.relay_url, ts.to_string()],
        )?;
        self.emit(
            identity,
            crate::crdt::CrdtTable::AppSettings,
            "settings",
            &Self::settings_json(s),
        )?;
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

    /// Write-through helper: encrypt + enqueue the canonical JSON for a
    /// just-written row. No-op when `identity` is `None` (offline vault-
    /// locked contexts and unit tests that assert storage only).
    /// Table names go through the CRDT registry — never raw strings.
    fn emit(
        &self,
        identity: Option<&Identity>,
        table: crate::crdt::CrdtTable,
        record_id: &str,
        payload: &serde_json::Value,
    ) -> Result<(), StoreError> {
        if let Some(idn) = identity {
            self.enqueue_outbox(idn, table.as_str(), record_id, payload)?;
        }
        Ok(())
    }

    /// Enqueue an encrypted CRDT op for a row mutation. AAD binds the
    /// ciphertext to its routing header (table:record).
    pub fn enqueue_outbox(
        &self,
        identity: &Identity,
        table_name: &str,
        record_id: &str,
        payload_json: &serde_json::Value,
    ) -> Result<String, StoreError> {
        let ts = self.tick();
        let op_id = new_id("op");
        let aad = format!("{table_name}:{record_id}");
        let plaintext = serde_json::to_vec(payload_json).expect("json ser");
        let sealed = aead::seal(identity, &plaintext, aad.as_bytes())
            .map_err(|e| StoreError::Invalid(e.to_string()))?;
        self.conn.lock().unwrap().execute(
            "INSERT INTO crdt_outbox (operation_id, hlc_timestamp, table_name, record_id, encrypted_payload, created_at_epoch_ms, pushed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)",
            params![op_id, ts.to_string(), table_name, record_id, sealed.to_bytes(), ts.epoch_ms() as i64],
        )?;
        Ok(op_id)
    }

    /// Pending (unpushed) outbox operations, oldest first. The tie-break
    /// on `(hlc_timestamp, operation_id)` is load-bearing: ops enqueued
    /// within the same wall millisecond share `created_at_epoch_ms`, and
    /// a bare `ORDER BY created_at_epoch_ms` lets SQLite return either
    /// order — retries could then re-push the same ops shuffled.
    pub fn pending_outbox(&self, limit: i64) -> Result<Vec<OutboxOp>, StoreError> {
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
        status: GoalStatus::from_str(&r.get::<_, String>(4)?).expect("db goal status"),
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
        status: MilestoneStatus::from_str(&r.get::<_, String>(5)?).expect("db milestone status"),
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
        state: DirectiveState::from_str(&r.get::<_, String>(7)?).expect("db directive state"),
        scheduled_for_date: r.get(8)?,
        hlc_timestamp: parse_hlc(&r.get::<_, String>(9)?)?,
    })
}
