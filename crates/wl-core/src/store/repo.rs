//! Repositories for every domain aggregate. Each write path also
//! enqueues the corresponding CRDT op into `crdt_outbox` (encrypted
//! with the identity's payload key) so sync is durable-by-default.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{params, Connection, OptionalExtension};

use crate::crypto::aead;
use crate::crypto::identity::Identity;
use crate::domain::*;
use crate::hlc::{Hlc, HlcTimestamp};
use crate::poison::lock_conn;

use super::StoreError;

/// The relay rejects any single sealed op larger than this
/// (`wl-protocol::MAX_SEALED_BYTES`, 256 KiB). Mirrored here as a
/// literal rather than a dependency so `wl-core` stays platform-clean
/// and dependency-light — but "the two must move together" is not
/// something a comment can enforce. `wl-sync` depends on BOTH crates
/// and holds a `const _: () = assert!(…)` over this value and
/// `wl_protocol::MAX_SEALED_BYTES`, so drift is a compile error at
/// the crate that would actually wedge sync.
///
/// Public only for that assert; not part of `wl-core`'s API surface.
pub const MAX_SEALED_OP_BYTES: usize = 256 * 1024;

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

/// Ceiling on how many phases one progressive directive may be split
/// into. A guard, not a product rule: `progressive_total` is a column
/// a replicated op writes with no schema constraint, and
/// `ensure_phases` sizes a `Vec` and a loop from it on a path the
/// engine hits on every canvas load. The planner emits 2-4.
const MAX_PROGRESSIVE_PHASES: i64 = 64;

fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4().simple())
}

/// SQLite has no unsigned 64-bit integer, and the HLC physical
/// component is nanoseconds since the UNIX epoch — a `u64` that can
/// outrun `i64::MAX` by a factor of two.
///
/// `as i64` REINTERPRETS rather than converts, so a wall component past
/// `i64::MAX` (the year ~2262, or any host clock set past it) is stored
/// as a NEGATIVE number. That value is a landmine in a column every
/// comparison reads: `> 0` is false, ordering inverts, and the round
/// trip only survives by accident of two's complement. Saturating keeps
/// the column a real timestamp. The clamp loses at most the difference
/// between the two, and `now()` treats the wall clock as authoritative
/// anyway (`wall > head` resets the counter), so nothing regresses.
fn nanos_to_i64(nanos: u64) -> i64 {
    i64::try_from(nanos).unwrap_or(i64::MAX)
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

    /// The single sanctioned way to reach the SQLite handle.
    ///
    /// Recovers from mutex poisoning rather than failing the process
    /// (SHELL-1) and rolls back any transaction a panicking writer left
    /// open. Prefer this over touching [`Repos::conn`] directly — the
    /// engine and the sync layer both did the latter, which is exactly
    /// how the poison-panic sites accumulated (CORE-7f).
    pub fn lock_conn(&self) -> MutexGuard<'_, Connection> {
        lock_conn(&self.conn)
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
        let conn = self.lock_conn();
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
        let conn = self.lock_conn();
        let (wall, ctr) = self.hlc.head();
        conn.execute(
            "INSERT INTO hlc_clock (id, last_wall_nanos, counter, device) VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET last_wall_nanos=?1, counter=?2, device=?3",
            params![nanos_to_i64(wall), ctr as i64, self.device as i64],
        )?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Identity
    // ------------------------------------------------------------------

    /// Persists the public identity half after onboarding.
    /// `verify_indices`: the 3-word backup-challenge positions, so the
    /// authoritative re-check stays positional across restarts.
    ///
    /// `identity_config` is a singleton keyed on `id = 1` (CORE-4), so a
    /// restore with a *different* phrase replaces the row rather than
    /// appending a second one. It is a plain `INSERT` with no
    /// `ON CONFLICT`: the previous `ON CONFLICT(public_key) DO UPDATE`
    /// only deduplicated the *same* key and silently accumulated
    /// identities, leaving `identity()`'s `LIMIT 1` an arbitrary pick.
    /// Replace-then-insert inside one transaction keeps the singleton
    /// intact if the insert ever fails.
    pub fn insert_identity(
        &self,
        identity: &Identity,
        verified: bool,
        verify_indices: &[usize],
    ) -> Result<(), StoreError> {
        let ts = self.tick();
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM identity_config", [])?;
        tx.execute(
            "INSERT INTO identity_config (id, public_key, bip39_mnemonic_verified, verify_indices, hlc_timestamp)
             VALUES (1, ?1, ?2, ?3, ?4)",
            params![
                identity.account_id_hex(),
                verified,
                serde_json::to_string(verify_indices).expect("json indices"),
                ts.to_string()
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn identity(&self) -> Result<Option<IdentityConfig>, StoreError> {
        self.lock_conn()
            .query_row(
                "SELECT public_key, bip39_mnemonic_verified, verify_indices, hlc_timestamp
                 FROM identity_config WHERE id = 1",
                [],
                |r| {
                    Ok(IdentityConfig {
                        public_key: r.get(0)?,
                        bip39_mnemonic_verified: r.get(1)?,
                        // A corrupt `verify_indices` is a CORRUPTION, not
                        // "no challenge". `unwrap_or_default` reported an
                        // empty challenge list, so the shell treated every
                        // backup challenge as invalid and silently
                        // re-prompted onboarding, with the bad row
                        // invisible. Every other mapper here surfaces its
                        // parse failure; so does this one.
                        verify_indices: serde_json::from_str(&r.get::<_, String>(2)?).map_err(
                            |e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    2,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            },
                        )?,
                        hlc_timestamp: parse_hlc(&r.get::<_, String>(3)?)?,
                    })
                },
            )
            .optional()
            .map_err(StoreError::Sqlite)
    }

    /// Persists the backup-challenge verification result.
    ///
    /// Targets `id = 1` explicitly (CORE-4). With the old multi-row
    /// schema this bare `UPDATE` had no `WHERE`, so it stamped the flag
    /// onto *every* stored identity at once.
    pub fn set_mnemonic_verified(&self, verified: bool) -> Result<(), StoreError> {
        let ts = self.tick();
        self.lock_conn().execute(
            "UPDATE identity_config SET bip39_mnemonic_verified = ?1, hlc_timestamp = ?2
             WHERE id = 1",
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
        complexity: i64,
        identity: Option<&Identity>,
    ) -> Result<Goal, StoreError> {
        check_text("goal title", title, MAX_TITLE_CHARS).map_err(StoreError::Invalid)?;
        check_optional_text("goal description", description, MAX_DESCRIPTION_CHARS)
            .map_err(StoreError::Invalid)?;
        if let Some(d) = target_date {
            check_date("goal target date", d).map_err(StoreError::Invalid)?;
        }
        crate::domain::check_complexity("goal complexity", complexity)
            .map_err(StoreError::Invalid)?;
        let id = new_id("goal");
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        let ts = self.hlc.now(self.device);
        let goal = Goal {
            id: id.clone(),
            title: title.to_string(),
            description: description.map(Into::into),
            target_date: target_date.map(Into::into),
            status: GoalStatus::Active,
            complexity,
            hlc_timestamp: ts,
        };
        tx.execute(
            "INSERT INTO goals (id, title, description, target_date, status, complexity, hlc_timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                title,
                description,
                target_date,
                "active",
                complexity,
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
        self.lock_conn()
            .query_row(
                &format!("SELECT {GOAL_COLUMNS} FROM goals WHERE status = 'active' ORDER BY hlc_timestamp LIMIT 1"),
                [],
                goal_row,
            )
            .optional()
            .map_err(StoreError::Sqlite)
    }

    pub fn goal(&self, id: &str) -> Result<Option<Goal>, StoreError> {
        self.lock_conn()
            .query_row(
                &format!("SELECT {GOAL_COLUMNS} FROM goals WHERE id = ?1"),
                [id],
                goal_row,
            )
            .optional()
            .map_err(StoreError::Sqlite)
    }

    /// Every `active` goal with its milestone completion counts,
    /// newest first.
    ///
    /// Aggregated in SQL rather than by looping
    /// [`Repos::milestones_for_goal`]: a control panel lists all goals
    /// at once, and the N+1 form is a round trip per goal. The LEFT
    /// JOIN is load-bearing — a goal the user has not planned yet must
    /// still appear, with zeroes, or the panel hides a brand-new goal.
    ///
    /// `hlc_timestamp` is fixed-width zero-padded text, so the DESC
    /// sort is chronological, not lexicographic; the `g.id` tail keeps
    /// a same-instant pair (a restore can reissue one) from swapping
    /// between renders.
    pub fn active_goals_with_progress(&self) -> Result<Vec<GoalProgress>, StoreError> {
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT g.id, g.title, g.target_date,
                    COUNT(m.id) AS milestones_total,
                    COALESCE(SUM(CASE WHEN m.status = 'completed' THEN 1 ELSE 0 END), 0)
                        AS milestones_done
             FROM goals g
             LEFT JOIN milestones m ON m.goal_id = g.id
             WHERE g.status = 'active'
             GROUP BY g.id
             ORDER BY g.hlc_timestamp DESC, g.id ASC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                Ok(GoalProgress {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    target_date: r.get(2)?,
                    milestones_done: r.get::<_, i64>(4)? as usize,
                    milestones_total: r.get::<_, i64>(3)? as usize,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
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
        let mut conn = self.lock_conn();
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
        let conn = self.lock_conn();
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
        let mut conn = self.lock_conn();
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

    /// Sets a goal's lifecycle status (CORE-3).
    ///
    /// `GoalStatus` existed but had no writer, so every goal stayed
    /// `active` forever and `active_goal()`'s `LIMIT 1` was an
    /// arbitrary tie-break. Park a goal explicitly when it is achieved
    /// or superseded.
    pub fn set_goal_status(
        &self,
        id: &str,
        status: GoalStatus,
        identity: Option<&Identity>,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        let ts = self.hlc.now(self.device);
        if tx.execute(
            "UPDATE goals SET status = ?2, hlc_timestamp = ?3 WHERE id = ?1",
            params![id, status.as_str(), ts.to_string()],
        )? == 0
        {
            return Err(StoreError::NotFound(format!("goal {id}")));
        }
        if identity.is_some() {
            let g = tx.query_row(
                &format!("SELECT {GOAL_COLUMNS} FROM goals WHERE id = ?1"),
                [id],
                goal_row,
            )?;
            Self::emit_on(
                &tx,
                identity,
                crate::crdt::CrdtTable::Goals,
                &g.id,
                &Self::goal_json(&g),
                ts,
            )?;
        }
        self.persist_head_on(&tx)?;
        tx.commit()?;
        Ok(())
    }

    /// Parks every `active` goal except `keep` (CORE-3).
    ///
    /// Returns the ids it archived. Used when a new plan replaces an
    /// existing goal so multiple `active` goals cannot accumulate — the
    /// single-active invariant that holds for directives has to hold for
    /// goals too, or `active_goal()` has to break ties arbitrarily.
    pub fn archive_other_active_goals(
        &self,
        keep: &str,
        identity: Option<&Identity>,
    ) -> Result<Vec<String>, StoreError> {
        let stale: Vec<String> = {
            let conn = self.lock_conn();
            let mut stmt = conn.prepare(
                "SELECT id FROM goals WHERE status = 'active' AND id <> ?1 ORDER BY hlc_timestamp",
            )?;
            let rows = stmt.query_map([keep], |r| r.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        for id in &stale {
            self.set_goal_status(id, GoalStatus::Archived, identity)?;
        }
        Ok(stale)
    }

    /// The next `pending` milestone (lowest order_index) for a goal.
    pub fn next_pending_milestone(&self, goal_id: &str) -> Result<Option<Milestone>, StoreError> {
        self.lock_conn()
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
        self.lock_conn()
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
        depends_on: Option<&str>,
        phases: &[(String, Option<String>, i64)], // (title, instruction, minutes)
        identity: Option<&Identity>,
    ) -> Result<Directive, StoreError> {
        check_text("directive title", title, MAX_TITLE_CHARS).map_err(StoreError::Invalid)?;
        check_optional_text("directive context", execution_context, MAX_CONTEXT_CHARS)
            .map_err(StoreError::Invalid)?;
        check_minutes("directive estimate", estimated_minutes).map_err(StoreError::Invalid)?;
        check_date("directive date", scheduled_for_date).map_err(StoreError::Invalid)?;
        check_optional_text("prerequisite", depends_on, MAX_TITLE_CHARS)
            .map_err(StoreError::Invalid)?;
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
        let mut conn = self.lock_conn();
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
            depends_on: depends_on.map(str::to_string),
            hlc_timestamp: ts,
        };
        tx.execute(
            "INSERT INTO directives (id, milestone_id, title, execution_context,
                estimated_minutes, progressive_step, progressive_total, state, scheduled_for_date,
                depends_on, hlc_timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, 'queued', ?7, ?8, ?9)",
            params![id, milestone_id, title, execution_context, estimated_minutes, progressive_total, scheduled_for_date, depends_on, ts.to_string()],
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
                &crate::crdt::phase_record_id(&id, p.step),
                &Self::phase_json(&p),
                ts,
            )?;
        }
        self.persist_head_on(&tx)?;
        tx.commit()?;
        Ok(d)
    }

    pub fn directive(&self, id: &str) -> Result<Option<Directive>, StoreError> {
        Self::directive_on(&self.lock_conn(), id)
    }

    /// Returns the directive's phase rows, rebuilding them from the
    /// directive's own estimate if any are missing (CORE-6).
    ///
    /// `current_phase_minutes` used to fail closed with
    /// `Invalid("current phase missing")` whenever a progressive
    /// directive lost a phase row — a truncated pull, a partial
    /// `create_directive`, or a hand-edited data dir. That error broke
    /// `activate_next`, `complete` and the HUD with no way forward: the
    /// user could neither run nor requeue the directive.
    ///
    /// The repair is deterministic and lossless by construction: phases
    /// are `progressive_total` equal slices of `estimated_minutes` (floor
    /// 1 minute each, remainder to the last step) so the rows always sum
    /// back to the estimate. A monolithic directive (`progressive_total
    /// <= 1`) legitimately has no rows and returns an empty vec without
    /// writing anything.
    ///
    /// Rebuilt rows go through the normal write path — fresh HLC, an
    /// emitted CRDT op when an identity is present, and `record_heads` —
    /// so the repair replicates to other devices instead of silently
    /// diverging.
    pub fn ensure_phases(
        &self,
        directive_id: &str,
        identity: Option<&Identity>,
    ) -> Result<Vec<DirectivePhase>, StoreError> {
        let existing = self.phases_for_directive(directive_id)?;
        let d = self
            .directive(directive_id)?
            .ok_or_else(|| StoreError::NotFound(format!("directive {directive_id}")))?;
        if d.progressive_total <= 1 {
            return Ok(existing);
        }
        if existing.len() as i64 == d.progressive_total
            && (1..=d.progressive_total)
                .all(|step| existing.iter().any(|p| p.step == step && p.minutes > 0))
        {
            return Ok(existing);
        }

        let count = d.progressive_total;
        // `progressive_total` arrives from a replicated row with no
        // schema bound, and the only guard below was
        // `estimated_minutes >= count`. One op carrying
        // `estimated_minutes = 10^18, progressive_total = 10^9` cleared
        // that and reached `Vec::with_capacity(count)` — a multi-gigabyte
        // allocation on the path the engine runs on every canvas load.
        // The real product caps a progressive directive at a handful of
        // phases (`Directive::PROGRESSIVE_THRESHOLD_MINUTES` is a
        // duration, not a count; the AI planner emits 2-4).
        if !(1..=MAX_PROGRESSIVE_PHASES).contains(&count) {
            return Err(StoreError::Invalid(format!(
                "directive {directive_id} is corrupt: {count} phases is outside the \
                 supported range 1..={MAX_PROGRESSIVE_PHASES} — reschedule it"
            )));
        }
        if d.estimated_minutes < count {
            // Cannot split `estimated_minutes` into `count` positive
            // phases. Repairing would mean inventing time the user never
            // estimated, so fail closed — but with an error that names
            // the actual remedy instead of the old bare "current phase
            // missing".
            return Err(StoreError::Invalid(format!(
                "directive {directive_id} is corrupt: {count} phases cannot fit in a \
                 {}-minute estimate — reschedule it",
                d.estimated_minutes
            )));
        }
        // Equal slices, remainder to the last step: sums exactly, and
        // identical on every device that runs the repair. This is the
        // recipe for a step that has to be INVENTED, not a rule to
        // impose on one that already exists.
        let base = d.estimated_minutes / count;
        let remainder = d.estimated_minutes % count;
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        let mut rebuilt = Vec::with_capacity(count as usize);
        for step in 1..=count {
            // A step that already exists keeps EVERYTHING, minutes
            // included.
            //
            // The repair used to recompute equal slices for all of them,
            // so a directive authored as 5 + 25 minutes came back as
            // 15 + 15 the first time any phase row went missing — and
            // because the local `ON CONFLICT` clause only touched
            // `minutes`, the emitted op carried a SYNTHESIZED row
            // (`title = "Step N"`, `instruction = None`, `state =
            // active|pending`) under a fresh, winning HLC. The peer lost
            // the real title, the instruction and any `done` state, and
            // the repair manufactured the divergence it exists to heal.
            // It ran on every canvas load, so one truncated pull was
            // enough to trigger it.
            let prior = existing.iter().find(|p| p.step == step);
            let p = match prior {
                Some(prev) => {
                    rebuilt.push(prev.clone());
                    continue;
                }
                None => DirectivePhase {
                    directive_id: directive_id.to_string(),
                    step,
                    title: format!("Step {step}"),
                    instruction: None,
                    minutes: if step == count {
                        base + remainder
                    } else {
                        base
                    },
                    state: if step == 1 {
                        PhaseState::Active
                    } else {
                        PhaseState::Pending
                    },
                    hlc_timestamp: self.hlc.now(self.device),
                },
            };
            let ts = p.hlc_timestamp;
            tx.execute(
                "INSERT INTO directive_phases (directive_id, step, title, instruction, minutes, state, hlc_timestamp)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(directive_id, step) DO UPDATE SET
                    minutes = excluded.minutes, hlc_timestamp = excluded.hlc_timestamp",
                params![
                    p.directive_id,
                    p.step,
                    p.title,
                    p.instruction,
                    p.minutes,
                    p.state.as_str(),
                    ts.to_string()
                ],
            )?;
            if identity.is_some() {
                Self::emit_on(
                    &tx,
                    identity,
                    crate::crdt::CrdtTable::DirectivePhases,
                    &crate::crdt::phase_record_id(directive_id, p.step),
                    &Self::phase_json(&p),
                    ts,
                )?;
            }
            rebuilt.push(p);
        }
        self.persist_head_on(&tx)?;
        tx.commit()?;
        Ok(rebuilt)
    }

    fn directive_on(conn: &Connection, id: &str) -> Result<Option<Directive>, StoreError> {
        conn.query_row(
            &format!("SELECT {DIRECTIVE_COLUMNS} FROM directives WHERE id = ?1"),
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
        self.lock_conn()
            .query_row(
                &format!(
                    "SELECT {DIRECTIVE_COLUMNS} FROM directives WHERE state = 'active'
                     ORDER BY hlc_timestamp DESC, id ASC LIMIT 1"
                ),
                [],
                directive_row,
            )
            .optional()
            .map_err(StoreError::Sqlite)
    }

    /// Upper bound on the number of directive rows the runnable-filter's
    /// state map will hold in memory at once.
    ///
    /// `MAX_DEPENDENCY_NODES` is a *guard*, not a real limit: a plan is
    /// capped at 5 milestones × 32 directives, so a legitimate table is
    /// 160 rows. It exists because the filter below reads the whole table
    /// once per canvas load, and `progressive_total` has already proved
    /// that a replicated integer will happily arrive as 10^9.
    const MAX_DEPENDENCY_NODES: usize = 4096;

    /// `id -> state` for every directive, or `None` past the guard above.
    ///
    /// One pass rather than a query per candidate: `runnable_directives`
    /// runs on the canvas's hot path, and a per-row lookup would make
    /// activation O(n) round trips through a `Mutex<Connection>`.
    fn directive_states(conn: &Connection) -> Result<Option<HashMap<String, String>>, StoreError> {
        let mut stmt = conn.prepare("SELECT id, state FROM directives")?;
        let mut rows = stmt.query([])?;
        let mut map = HashMap::new();
        while let Some(row) = rows.next()? {
            if map.len() >= Self::MAX_DEPENDENCY_NODES {
                return Ok(None);
            }
            map.insert(row.get::<_, String>(0)?, row.get::<_, String>(1)?);
        }
        Ok(Some(map))
    }

    /// Whether `depends_on` is clear for `d` to run.
    ///
    /// **One hop, deliberately.** A directive's prerequisite being
    /// `completed` is the whole condition, and the transitive part takes
    /// care of itself: the engine only ever *activates* a directive whose
    /// prerequisite is done, so `b completed` already implies `a
    /// completed` in every state the engine can produce. An earlier version
    /// of this walked the chain and returned true on the first `completed`
    /// ancestor, which made a three-deep chain unlock its tail the moment
    /// the HEAD was finished — the tail ran with an unfinished middle.
    ///
    /// One hop also means there is no loop to bound, and that is the second
    /// reason for it: `depends_on` is a replicated column written verbatim
    /// from a sealed payload, so a peer (or a half-applied merge) can land
    /// `a -> b -> a` on a device, and the engine calls this on EVERY
    /// canvas load. With no walk there is nothing to hang, and a cycle is
    /// simply never satisfied — both nodes stay queued, which is the same
    /// place an unmet prerequisite puts them.
    ///
    /// A prerequisite naming a directive this device does not have counts
    /// as UNMET rather than met. A truncated pull must not be able to
    /// unlock work whose ordering evidence is missing, and "unmet"
    /// converges: once the peer's row arrives it is seen.
    fn prerequisites_met(states: &HashMap<String, String>, depends_on: Option<&str>) -> bool {
        match depends_on {
            None => true,
            Some(id) => states.get(id).map(String::as_str) == Some("completed"),
        }
    }

    /// Queued directives scheduled on or before `date` whose prerequisite
    /// is met (execution pool for today — NOT exposed as a list in the
    /// UI; engine only).
    pub fn runnable_directives(&self, date: &str) -> Result<Vec<Directive>, StoreError> {
        let conn = self.lock_conn();
        let Some(states) = Self::directive_states(&conn)? else {
            return Err(StoreError::Invalid(
                "too many directives to resolve the dependency order".into(),
            ));
        };
        let mut stmt = conn.prepare(&format!(
            "SELECT {DIRECTIVE_COLUMNS} FROM directives
             WHERE state = 'queued' AND scheduled_for_date <= ?1
             ORDER BY scheduled_for_date, hlc_timestamp, id"
        ))?;
        let rows = stmt
            .query_map([date], directive_row)?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);
        Ok(rows
            .into_iter()
            .filter(|d| Self::prerequisites_met(&states, d.depends_on.as_deref()))
            .collect())
    }

    pub fn next_runnable_directive(&self, date: &str) -> Result<Option<Directive>, StoreError> {
        Ok(self.runnable_directives(date)?.into_iter().next())
    }

    /// Every goal's `(id, complexity)`, oldest first.
    ///
    /// The calibration estimator's starting set: a bucket exists because a
    /// goal rated it. Ordered by HLC so `calibration`'s output is stable
    /// across calls — the BTreeMap keys are stable already, but the
    /// *insertion* order decides which `Bucket::prior` seeds an entry, and
    /// two goals at the same rating seed it identically, so this is
    /// belt-and-braces against a future change that makes them differ.
    pub fn goals_by_complexity(&self) -> Result<Vec<Goal>, StoreError> {
        self.lock_conn()
            .prepare(&format!(
                "SELECT {GOAL_COLUMNS} FROM goals ORDER BY hlc_timestamp, id"
            ))?
            .query_map([], goal_row)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::Sqlite)
    }

    /// Every directive's `(goal id, goal complexity, state,
    /// scheduled_for_date)`.
    ///
    /// The estimator's evidence set. It joins through `milestones` because
    /// the rating lives on the GOAL — a task is "heavy" because the
    /// objective it belongs to is — and it carries the goal id so the
    /// caller can group per goal before rolling up by rating. (Rolling up
    /// first and then adding the total once per goal is a double count; see
    /// the note on `engine::calibration::calibration`.)
    ///
    /// No date filter: the estimator wants the whole record, not a window.
    /// A 14-day window is right for velocity (which is about pace) and
    /// wrong here, where a single heavy goal from two years ago is still
    /// the best evidence anyone has about heavy work.
    pub fn directive_evidence(
        &self,
    ) -> Result<Vec<(String, i64, DirectiveState, String)>, StoreError> {
        self.lock_conn()
            .prepare(
                "SELECT g.id, g.complexity, d.state, d.scheduled_for_date
                 FROM directives d
                 JOIN milestones m ON m.id = d.milestone_id
                 JOIN goals g ON g.id = m.goal_id",
            )?
            .query_map([], |r| {
                let goal_id: String = r.get(0)?;
                let complexity = crate::domain::clamp_complexity(r.get::<_, i64>(1)?);
                let state = parse_enum(
                    "directive state",
                    &r.get::<_, String>(2)?,
                    DirectiveState::from_str,
                )?;
                Ok((goal_id, complexity, state, r.get(3)?))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::Sqlite)
    }

    /// The directives named by `ids` that are not yet `completed`.
    ///
    /// The ledger's "what is blocking this" line. Ids this device does not
    /// have are reported as blocking rather than dropped, for the same
    /// reason `prerequisites_met` treats a missing prerequisite as unmet:
    /// silence would read as "nothing blocks this", which is the one answer
    /// a person cannot act on.
    pub fn unmet_prerequisites(
        &self,
        ids: &[String],
    ) -> Result<HashMap<String, Vec<String>>, StoreError> {
        let conn = self.lock_conn();
        let mut out = HashMap::new();
        for id in ids {
            let Some(dep) = (|| {
                conn.query_row(
                    "SELECT depends_on FROM directives WHERE id = ?1",
                    [id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()
                .map_err(StoreError::Sqlite)
            })()?
            else {
                continue;
            };
            let Some(dep) = dep else { continue };
            let state: Option<String> = conn
                .query_row("SELECT state FROM directives WHERE id = ?1", [&dep], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(StoreError::Sqlite)?;
            if state.as_deref() != Some("completed") {
                out.insert(id.clone(), vec![dep]);
            }
        }
        Ok(out)
    }

    pub fn set_directive_state(
        &self,
        id: &str,
        state: DirectiveState,
        identity: Option<&Identity>,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock_conn();
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
            params![nanos_to_i64(wall), ctr as i64, self.device as i64],
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
                &format!("SELECT {DIRECTIVE_COLUMNS} FROM directives WHERE id = ?1"),
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

    // ------------------------------------------------------------------
    // Deletes — tombstone writers (B-003).
    //
    // Every delete hard-removes the local row AND enqueues a tombstone
    // op (sealed `{"__tombstone": true}`) so the delete replicates;
    // row + op + clock head commit atomically like every other mutation
    // (#56). Deletes are leaf-first: FKs are enforced, so deleting a
    // goal/milestone/directive that still has children fails with a
    // constraint error instead of orphaning rows — delete phases, then
    // directives, then milestones, then goals. Deleting a missing row
    // is `NotFound` and enqueues nothing (no local change → no op).
    // ------------------------------------------------------------------

    /// Deletes one row and replicates the delete as a tombstone op.
    fn delete_record(
        &self,
        table: crate::crdt::CrdtTable,
        record_id: &str,
        delete_sql: &str,
        identity: Option<&Identity>,
    ) -> Result<(), StoreError> {
        if record_id.is_empty() || record_id.len() > 128 {
            return Err(StoreError::Invalid("bad CRDT record id".into()));
        }
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        let ts = self.hlc.now(self.device);
        let removed = tx.execute(delete_sql, params![record_id])?;
        if removed == 0 {
            return Err(StoreError::NotFound(format!(
                "{} {record_id}",
                table.as_str()
            )));
        }
        let payload = serde_json::json!({ crate::crdt::TOMBSTONE_MARKER: true });
        let op_id = if let Some(identity) = identity {
            Some(Self::enqueue_on(
                &tx,
                identity,
                table.as_str(),
                record_id,
                &payload,
                ts,
            )?)
        } else {
            None
        };
        // Local delete memory: an older upsert arriving later must not
        // resurrect the row (mirrors `TableState.tombstones`). Without
        // an outbox op there is no op id; the tick itself is unique and
        // monotonic per repo, so it serves as the local-only key.
        let ts_s = ts.to_string();
        tx.execute(
            "INSERT INTO record_heads (table_name, record_id, hlc_timestamp, device, operation_id, tombstone)
             VALUES (?1, ?2, ?3, ?4, ?5, 1)
             ON CONFLICT(table_name, record_id) DO UPDATE SET hlc_timestamp=?3, device=?4, operation_id=?5, tombstone=1",
            params![
                table.as_str(),
                record_id,
                ts_s,
                ts.device as i64,
                op_id.as_deref().unwrap_or(&ts_s)
            ],
        )?;
        self.persist_head_on(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn delete_goal(&self, id: &str, identity: Option<&Identity>) -> Result<(), StoreError> {
        self.delete_record(
            crate::crdt::CrdtTable::Goals,
            id,
            "DELETE FROM goals WHERE id = ?1",
            identity,
        )
    }

    pub fn delete_milestone(
        &self,
        id: &str,
        identity: Option<&Identity>,
    ) -> Result<(), StoreError> {
        self.delete_record(
            crate::crdt::CrdtTable::Milestones,
            id,
            "DELETE FROM milestones WHERE id = ?1",
            identity,
        )
    }

    pub fn delete_directive(
        &self,
        id: &str,
        identity: Option<&Identity>,
    ) -> Result<(), StoreError> {
        self.delete_record(
            crate::crdt::CrdtTable::Directives,
            id,
            "DELETE FROM directives WHERE id = ?1",
            identity,
        )
    }

    /// Deletes one progressive phase step. Phase ops share the
    /// directive-level record id (one op per step, mirroring
    /// `create_directive`'s emit granularity).
    pub fn delete_directive_phase(
        &self,
        directive_id: &str,
        step: i64,
        identity: Option<&Identity>,
    ) -> Result<(), StoreError> {
        if directive_id.is_empty() || directive_id.len() > 128 {
            return Err(StoreError::Invalid("bad CRDT record id".into()));
        }
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        let ts = self.hlc.now(self.device);
        let removed = tx.execute(
            "DELETE FROM directive_phases WHERE directive_id = ?1 AND step = ?2",
            params![directive_id, step],
        )?;
        if removed == 0 {
            return Err(StoreError::NotFound(format!(
                "directive_phases {directive_id}:{step}"
            )));
        }
        let payload = serde_json::json!({ crate::crdt::TOMBSTONE_MARKER: true });
        let op_id = if let Some(identity) = identity {
            Some(Self::enqueue_on(
                &tx,
                identity,
                crate::crdt::CrdtTable::DirectivePhases.as_str(),
                &crate::crdt::phase_record_id(directive_id, step),
                &payload,
                ts,
            )?)
        } else {
            None
        };
        let ts_s = ts.to_string();
        tx.execute(
            "INSERT INTO record_heads (table_name, record_id, hlc_timestamp, device, operation_id, tombstone)
             VALUES ('directive_phases', ?1, ?2, ?3, ?4, 1)
             ON CONFLICT(table_name, record_id) DO UPDATE SET hlc_timestamp=?2, device=?3, operation_id=?4, tombstone=1",
            params![crate::crdt::phase_record_id(directive_id, step), ts_s, ts.device as i64, op_id.as_deref().unwrap_or(&ts_s)],
        )?;
        self.persist_head_on(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn delete_check_in(&self, id: &str, identity: Option<&Identity>) -> Result<(), StoreError> {
        self.delete_record(
            crate::crdt::CrdtTable::CheckIns,
            id,
            "DELETE FROM check_ins WHERE id = ?1",
            identity,
        )
    }

    // ------------------------------------------------------------------
    // Task ledger (2026-09-27)
    // ------------------------------------------------------------------

    /// Every task on the ledger, grouped-ready for the Entropy Log.
    ///
    /// Open work first, then the tasks already finished, and within each
    /// the store's own ordering (scheduled date, then HLC). The UI groups
    /// by goal; doing that client-side would need a second round trip per
    /// row, so the join goes all the way up to the goal here.
    ///
    /// `blocked_by` is the unmet prerequisite's *title*, joined in. It is
    /// the one field that makes this page worth more than a list: a task
    /// sitting in the queue with nothing obviously wrong with it is
    /// indistinguishable from a stalled app without it, and the answer
    /// ("waiting on Draft the outline") is a sentence the user can act on.
    ///
    /// Inner joins are safe because `foreign_keys = ON` makes the
    /// directives→milestones→goals chain unbreakable — a deleted parent
    /// would have blocked the delete, not orphaned a row.
    ///
    /// `limit` is clamped rather than rejected, like the outbox reader's
    /// sibling: a page size is a display choice, so an over-large request
    /// widens to the cap instead of surfacing an error the page can do
    /// nothing about.
    pub fn task_ledger(&self, limit: usize) -> Result<Vec<LedgerRow>, StoreError> {
        let limit = limit.min(MAX_LEDGER_ROWS) as i64;
        let conn = self.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT d.id, g.id, g.title, g.complexity, m.id, m.title, d.title,
                    d.execution_context, d.estimated_minutes, d.state,
                    d.scheduled_for_date, d.progressive_step, d.progressive_total,
                    (SELECT p.title FROM directives p WHERE p.id = d.depends_on) AS blocker
             FROM directives d
             JOIN milestones m ON m.id = d.milestone_id
             JOIN goals g ON g.id = m.goal_id
             ORDER BY (d.state = 'completed'), d.scheduled_for_date, d.hlc_timestamp, d.id
             LIMIT ?1",
        )?;
        let rows = stmt
            .query_map([limit], |r| {
                Ok(LedgerRow {
                    directive_id: r.get(0)?,
                    goal_id: r.get(1)?,
                    goal_title: r.get(2)?,
                    complexity: crate::domain::clamp_complexity(r.get(3)?),
                    milestone_id: r.get(4)?,
                    milestone_title: r.get(5)?,
                    title: r.get(6)?,
                    execution_context: r.get(7)?,
                    estimated_minutes: r.get(8)?,
                    state: parse_enum(
                        "directive state",
                        &r.get::<_, String>(9)?,
                        DirectiveState::from_str,
                    )?,
                    scheduled_for_date: r.get(10)?,
                    phase: (r.get::<_, i64>(11)?, r.get::<_, i64>(12)?),
                    blocked_by: r.get::<_, Option<String>>(13)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Reconciles the single-active invariant after a pull-apply batch:    /// LWW arbitration can merge `active` states from two devices. The
    /// winner is deterministic across replicas (max `hlc_timestamp`,
    /// tie-break min `id`); losers are parked to queued with
    /// write-through. Returns the number parked.
    pub fn enforce_single_active(&self, identity: Option<&Identity>) -> Result<usize, StoreError> {
        let conn = self.lock_conn();
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
            params![nanos_to_i64(wall), ctr as i64, self.device as i64],
        )?;
        tx.commit()?;
        Ok(parked)
    }

    /// Sets (or clears) a directive's single prerequisite.
    ///
    /// A second pass rather than a parameter on [`Self::create_directive`]
    /// alone, because the plan's edges are expressed as indices into a list
    /// the model is still writing: every directive has to exist before any
    /// of them can be named. The caller creates first, then links.
    ///
    /// Refuses a self-reference and a two-node cycle here rather than
    /// leaving it to `prerequisites_met`, because these are *user* edits on
    /// the plan preview and the message is the feedback: a cycle is a thing
    /// a person can do by tapping the wrong node, and the alternative is
    /// work that silently never becomes runnable.
    pub fn set_directive_depends_on(
        &self,
        id: &str,
        depends_on: Option<&str>,
        identity: Option<&Identity>,
    ) -> Result<(), StoreError> {
        check_optional_text("prerequisite", depends_on, MAX_TITLE_CHARS)
            .map_err(StoreError::Invalid)?;
        if let Some(dep) = depends_on {
            if dep == id {
                return Err(StoreError::Invalid("a task cannot depend on itself".into()));
            }
            if depends_on_reaches(&self.lock_conn(), dep, id, 0)? {
                return Err(StoreError::Invalid(
                    "that would make two tasks depend on each other".into(),
                ));
            }
        }
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM directives WHERE id = ?1)",
            [id],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(StoreError::NotFound(format!("directive {id}")));
        }
        let ts = self.hlc.now(self.device);
        tx.execute(
            "UPDATE directives SET depends_on = ?2, hlc_timestamp = ?3 WHERE id = ?1",
            params![id, depends_on, ts.to_string()],
        )?;
        let d = Self::directive_on(&tx, id)?
            .ok_or_else(|| StoreError::NotFound(format!("directive {id}")))?;
        Self::emit_on(
            &tx,
            identity,
            crate::crdt::CrdtTable::Directives,
            &d.id,
            &Self::directive_json(&d),
            ts,
        )?;
        self.persist_head_on(&tx)?;
        tx.commit()?;
        Ok(())
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
        let mut conn = self.lock_conn();
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
                    // CORE-7(e) / AUDIT-6: `rounded` is non-negative and
                    // bounded by `estimated_minutes` (≤1440, validated
                    // above), so the `i128 -> i64` narrowing cannot
                    // overflow. Asserted rather than assumed: this is the
                    // repo's first `debug_assert`, and the convention
                    // going forward is that a narrowing cast on a
                    // user-influenced number states why it is safe.
                    debug_assert!(
                        rounded >= 0 && rounded <= i128::from(estimated_minutes),
                        "phase rescale out of range: {rounded} for estimate {estimated_minutes}"
                    );
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
                    &crate::crdt::phase_record_id(id, p.step),
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
        let mut conn = self.lock_conn();
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
                &crate::crdt::phase_record_id(id, p.step),
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

    /// Clamps `progressive_step` back into `1..=progressive_total` and
    /// writes it through.
    ///
    /// Needed because the two columns are separate integers with no
    /// cross-field CHECK, and the pull path writes both straight from a
    /// replicated payload. A `step` past the `total` has no phase row, so
    /// every engine path that reads the current phase failed and nothing
    /// could requeue the directive.
    pub fn reset_progressive_step(
        &self,
        id: &str,
        identity: Option<&Identity>,
    ) -> Result<(), StoreError> {
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        let mut d = Self::directive_on(&tx, id)?
            .ok_or_else(|| StoreError::NotFound(format!("directive {id}")))?;
        if d.progressive_total < 1 {
            return Err(StoreError::Invalid(format!("directive {id} has no phases")));
        }
        let clamped = d.progressive_step.clamp(1, d.progressive_total);
        if clamped == d.progressive_step {
            return Ok(());
        }
        d.progressive_step = clamped;
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
        self.persist_head_on(&tx)?;
        tx.commit()?;
        Ok(())
    }

    pub fn phases_for_directive(
        &self,
        directive_id: &str,
    ) -> Result<Vec<DirectivePhase>, StoreError> {
        Self::phases_on(&self.lock_conn(), directive_id)
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
        self.lock_conn()
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
        let mut conn = self.lock_conn();
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
        self.lock_conn()
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
        let conn = self.lock_conn();
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
    // Settings
    // ------------------------------------------------------------------

    pub fn settings(&self) -> Result<AppSettings, StoreError> {
        self.lock_conn()
            .query_row(
                "SELECT theme, ai_provider, tier1_model, tier2_model, relay_url
                 FROM app_settings WHERE id = 1",
                [],
                |r| {
                    Ok(AppSettings {
                        theme: r.get(0)?,
                        ai_provider: r.get(1)?,
                        tier1_model: r.get(2)?,
                        tier2_model: r.get(3)?,
                        relay_url: r.get(4)?,
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
        // CORE-7(a): validate the relay URL at the write boundary, not
        // only in the shell. `save_settings` is also reached by
        // pull-apply (a peer can set `relay_url` via a replicated
        // `app_settings` op), and the shell's pre-validation cannot see
        // that path — so a malicious or buggy peer could persist a
        // `file://` or metadata-endpoint URL that the next sync cycle
        // would then dial. `net::validate_relay_url` is the single
        // policy (SSRF blocklist, scheme/credential/query rules).
        if let Some(url) = s.relay_url.as_deref() {
            if !url.trim().is_empty() {
                crate::net::validate_relay_url(url).map_err(StoreError::Invalid)?;
            }
        }
        let mut conn = self.lock_conn();
        let tx = conn.transaction()?;
        let ts = self.hlc.now(self.device);
        // Two columns are deliberately absent from this statement and from
        // the SELECT in `settings()`: `hotkey` and `always_on_top`. Both
        // were features that no longer exist (a global summon hotkey; a
        // pin-above-other-windows preference). Neither column is dropped,
        // because dropping a column on a CRDT-synced singleton costs a
        // migration and buys nothing — each keeps its `NOT NULL DEFAULT`,
        // so omitting it here lets SQLite supply the default, and no reader
        // ever asks for it. An old peer's op payload that still carries an
        // `always_on_top` key is ignored on the pull side for the same
        // reason, so a mixed-version fleet converges on the new shape
        // without a migration or a rejection.
        tx.execute(
            "INSERT INTO app_settings (id, theme, ai_provider, tier1_model, tier2_model, relay_url, hlc_timestamp)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET theme=?1,
                ai_provider=?2, tier1_model=?3, tier2_model=?4, relay_url=?5, hlc_timestamp=?6",
            params![s.theme, s.ai_provider, s.tier1_model, s.tier2_model, s.relay_url, ts.to_string()],
        )?;
        Self::emit_on(
            &tx,
            identity,
            crate::crdt::CrdtTable::AppSettings,
            "settings",
            &Self::settings_json(s),
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
            "complexity": g.complexity,
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
            "depends_on": d.depends_on,
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

    // NOTE: there is deliberately no `bailouts_json`. The `bailouts` TABLE
    // is still in the schema (migration 0001) and still has a `CrdtTable`
    // arm on the sync side, because a device that has not been updated
    // keeps pushing its rows and quarantining them would be a louder
    // failure than ignoring them. Nothing WRITES one any more: the escape
    // hatch was deleted 2026-09-27 and `bail_out` was its only writer. See
    // delta 175 for why the column stayed while the feature went.

    fn settings_json(s: &AppSettings) -> serde_json::Value {
        serde_json::json!({
            "theme": s.theme,
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
        let op_id = match identity {
            Some(identity) => Some(Self::enqueue_on(
                conn,
                identity,
                table.as_str(),
                record_id,
                payload,
                ts,
            )?),
            None => None,
        };
        // Every LOCAL upsert must advance the record's merge head.
        //
        // Only the delete writers did, so the head lagged behind the row
        // on every ordinary write. The pull side prefers the head over
        // the row's own HLC (the `row_hlc` fallback only applies when no
        // head exists), which produced this: a peer op for `app_settings`
        // at T_r lands and plants head=T_r; the user changes a setting at
        // T_l > T_r, which updates the row but leaves head=T_r; a third
        // device's op at T_m between the two then beats the head, wins
        // arbitration, and blind-upserts the STALE value over the user's
        // newer edit. The head exists to be the record's true merge
        // position; a local write moves that position.
        //
        // Without an outbox op there is no operation id, so the tick
        // itself stands in — it is unique and monotonic per repo, which
        // is the same key the delete path uses.
        let ts_s = ts.to_string();
        conn.execute(
            "INSERT INTO record_heads (table_name, record_id, hlc_timestamp, device, operation_id, tombstone)
             VALUES (?1, ?2, ?3, ?4, ?5, 0)
             ON CONFLICT(table_name, record_id) DO UPDATE SET
                hlc_timestamp=?3, device=?4, operation_id=?5, tombstone=0",
            params![
                table.as_str(),
                record_id,
                ts_s,
                ts.device as i64,
                op_id.as_deref().unwrap_or(&ts_s)
            ],
        )?;
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
        let aad = crate::crdt::routing_aad(table_name, record_id, &op_id, &ts.to_string());
        let plaintext = zeroize::Zeroizing::new(
            serde_json::to_vec(payload_json).map_err(|e| StoreError::Invalid(e.to_string()))?,
        );
        let sealed = aead::seal(identity, &plaintext, aad.as_bytes())
            .map_err(|e| StoreError::Invalid(e.to_string()))?;
        // CORE-7(b): enforce the relay's per-op sealed cap at enqueue.
        // The relay rejects a >256 KiB sealed op with a 4xx, so an
        // oversized op written here is not merely rejected — it is
        // *undrainable*: it sits in the outbox forever, every drain
        // attempt fails the whole batch, and sync is wedged with no
        // local remedy. Failing the write is strictly better: the
        // caller's own row write rolls back in the same transaction, so
        // the store never holds state it cannot replicate. The
        // write-boundary text budgets (#64) keep normal content far
        // below this, so tripping it means something unbounded leaked in
        // and the error should be loud.
        if sealed.to_bytes().len() > MAX_SEALED_OP_BYTES {
            return Err(StoreError::Invalid(format!(
                "sealed op for {table_name}/{record_id} is {} bytes, over the \
                 {MAX_SEALED_OP_BYTES}-byte relay cap — the write was rolled back \
                 rather than enqueueing an undrainable op",
                sealed.to_bytes().len()
            )));
        }
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
            params![nanos_to_i64(wall), ctr as i64, self.device as i64],
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
        let mut conn = self.lock_conn();
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
        let conn = self.lock_conn();
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
        let mut conn = self.lock_conn();
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
            .lock_conn()
            .execute("DELETE FROM crdt_outbox WHERE pushed = 1", [])?;
        Ok(n)
    }

    // ------------------------------------------------------------------
    // Applied-op watermark (idempotent remote apply).
    // ------------------------------------------------------------------

    pub fn is_op_applied(&self, operation_id: &str) -> Result<bool, StoreError> {
        self.lock_conn()
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
        self.lock_conn().execute(
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
        let n = self.lock_conn().execute(
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

/// An active goal plus its milestone tally, for the control panel.
/// Counts are `usize` because they are display denominators: nothing
/// downstream does arithmetic that could overflow or go negative.
#[derive(Debug, Clone)]
pub struct GoalProgress {
    pub id: String,
    pub title: String,
    pub target_date: Option<String>,
    pub milestones_done: usize,
    pub milestones_total: usize,
}

/// One task on the Entropy Log, denormalised with the goal and milestone
/// it belongs to (2026-09-27).
///
/// This replaced `EntropyEntry`, which was a bailout row: the escape hatch
/// is gone, so the ledger's spine is now every task, and the only write is
/// a tick. The fields are the ones the page actually renders — nothing
/// here is a normalised reference, because every consumer wants the joined
/// text and a client-side lookup would be a second query per row.
#[derive(Debug, Clone)]
pub struct LedgerRow {
    pub directive_id: String,
    pub goal_id: String,
    pub goal_title: String,
    /// The goal's own *Estimated Complexity* rating, 1–5.
    pub complexity: i64,
    pub milestone_id: String,
    pub milestone_title: String,
    pub title: String,
    pub execution_context: Option<String>,
    pub estimated_minutes: i64,
    pub state: DirectiveState,
    pub scheduled_for_date: String,
    /// `(step, total)` — the phase badge the canvas shows, carried here so
    /// the ledger can say "phase 2 of 4" without a second round trip.
    pub phase: (i64, i64),
    /// The title of this task's prerequisite when that prerequisite is not
    /// `completed`, else `None`.
    ///
    /// The subquery is a correlated `SELECT` on `depends_on` and NOT a
    /// join: a task with no prerequisite must still appear, and an inner
    /// join would drop exactly the rows that need explaining most.
    /// `None` is therefore ambiguous between "blocked by nothing" and
    /// "blocked by a row this device never pulled", and the UI resolves it
    /// against `state` — the wording differs, the fact that it is blocked
    /// does not.
    pub blocked_by: Option<String>,
}

impl LedgerRow {
    /// Whether this task is waiting on something.
    ///
    /// A `completed` task is never blocked, whatever a stale `depends_on`
    /// says: the user ticked it, and re-deriving an outstanding obligation
    /// from a completed row would put work back on the board that nobody
    /// put there.
    pub fn is_blocked(&self) -> bool {
        self.state != DirectiveState::Completed && self.blocked_by.is_some()
    }

    /// The phase badge, or `None` for a monolithic task.
    ///
    /// The `step >= 1 && step <= total` filter is the same one the canvas
    /// applies: a replicated `progressive_step` past the total is a merge
    /// artefact, and rendering "Phase 9 of 4" is worse than rendering
    /// nothing.
    pub fn phase_view(&self) -> Option<(i64, i64)> {
        let (step, total) = self.phase;
        (total > 1 && step >= 1 && step <= total).then_some((step, total))
    }
}

/// Upper bound on [`Repos::task_ledger`]. Matches the 1024-row ceiling the
/// outbox and check-in readers already use: a local install's whole ledger
/// is far smaller, and the cap exists so a stray `usize::MAX` cannot ask
/// SQLite to materialise an unbounded result set.
const MAX_LEDGER_ROWS: usize = 1024;

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

/// Whether the chain starting at `from` arrives back at `target`.
///
/// Only ever used to refuse a **user** edit that would make two tasks
/// depend on each other — the plan preview lets a person re-parent a node by
/// tapping, and a cycle is a thing you can do by tapping the wrong one. The
/// engine itself never walks, so this is the only place a chain is followed.
///
/// Bounded by [`Repos::MAX_DEPENDENCY_NODES`] hops for the same reason: the
/// chain can already be circular before anyone tries to add to it. The
/// bound is belt-and-braces — a `visited` set would do — but it costs
/// nothing and makes "this cannot spin" true by construction rather than by
/// argument.
fn depends_on_reaches(
    conn: &Connection,
    from: &str,
    target: &str,
    hops: usize,
) -> Result<bool, StoreError> {
    if from == target {
        return Ok(true);
    }
    if hops >= Repos::MAX_DEPENDENCY_NODES {
        return Ok(true);
    }
    let next: Option<Option<String>> = conn
        .query_row("SELECT depends_on FROM directives WHERE id = ?1", [from], |r| {
            r.get::<_, Option<String>>(0)
        })
        .optional()
        .map_err(StoreError::Sqlite)?;
    match next {
        Some(Some(next)) => depends_on_reaches(conn, &next, target, hops + 1),
        _ => Ok(false),
    }
}

fn goal_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Goal> {
    Ok(Goal {
        id: r.get(0)?,
        title: r.get(1)?,
        description: r.get(2)?,
        target_date: r.get(3)?,
        status: parse_enum("goal status", &r.get::<_, String>(4)?, GoalStatus::from_str)?,
        complexity: crate::domain::clamp_complexity(r.get(5)?),
        hlc_timestamp: parse_hlc(&r.get::<_, String>(6)?)?,
    })
}

/// The goal column list, in `goal_row` order, shared by every read.
///
/// One constant rather than the five hand-copied lists this file used to
/// carry. Adding `complexity` meant editing all five, and the failure mode
/// of missing one is a `column index out of range` at runtime on whichever
/// path was missed — the class of bug this repo keeps paying for.
const GOAL_COLUMNS: &str =
    "id, title, description, target_date, status, complexity, hlc_timestamp";

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
        depends_on: r.get(9)?,
        hlc_timestamp: parse_hlc(&r.get::<_, String>(10)?)?,
    })
}

/// The directive column list, in `directive_row` order, shared by every
/// read. See [`GOAL_COLUMNS`] for why this is one constant.
const DIRECTIVE_COLUMNS: &str = "id, milestone_id, title, execution_context, estimated_minutes, \
     progressive_step, progressive_total, state, scheduled_for_date, depends_on, hlc_timestamp";
