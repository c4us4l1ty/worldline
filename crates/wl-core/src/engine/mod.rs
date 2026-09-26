//! Tier-3 Local Heuristic Engine (PRD §4): the offline Rust-native
//! brain. Owns the single-active-directive invariant (US-3),
//! progressive phase activation (§5.2), the frictionful escape hatch
//! state machine (§5.3), evening check-ins (§5.4), and velocity
//! recalibration.
//!
//! Pure synchronous logic over [`Repos`] — no timers, no async; the
//! Tauri shell drives wall-clock events.

pub mod velocity;

use crate::crypto::identity::Identity;
use crate::domain::*;
use crate::store::repo::Repos;
use crate::store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("store: {0}")]
    Store(#[from] StoreError),
    #[error("no runnable directive available")]
    NothingRunnable,
    #[error("milestone {0} not found")]
    MilestoneMissing(String),
}

/// What the canvas should render after an engine transition.
#[derive(Debug, Clone, PartialEq)]
pub enum EngineOutcome {
    /// A directive is now active; render it. Contains directive id,
    /// current phase info, and remaining estimated minutes.
    DirectiveActive {
        directive_id: String,
        milestone_id: String,
        /// None for monolithic; Some((step, total)) for progressive.
        phase: Option<(i64, i64)>,
        /// Timer target: estimated minutes for the current phase/body.
        estimated_minutes: i64,
    },
    /// Directive fully completed → next one unlocks.
    DirectiveCompleted { directive_id: String },
    /// Bailout recorded; directive dormant; recovery offered.
    BailedOut {
        directive_id: String,
        reason: BailoutReason,
        recovery: Option<RecoveryAction>,
    },
    /// Queue empty for today — calm idle state (never a backlog view).
    Idle,
    /// Check-in recorded.
    CheckedIn {
        date: String,
        outcome: CheckInOutcome,
    },
}

/// The low-cognitive recovery / advance action after a bailout (§5.3).
#[derive(Debug, Clone, PartialEq)]
pub enum RecoveryAction {
    /// Downsized scope re-queued for tomorrow with halved estimate.
    DownsizeAndRequeue {
        directive_id: String,
        new_minutes: i64,
    },
    /// Dormant until external dependency clears; engine advanced to
    /// the next unblocked thread.
    AdvanceUnblocked { next_directive_id: String },
    /// Energy depletion: low-cognitive maintenance task offered.
    LowCognitiveTask { directive_id: String },
}

pub struct Engine<'a> {
    repos: &'a Repos,
    identity: Option<&'a Identity>,
}

impl<'a> Engine<'a> {
    /// `identity`: when `Some`, every mutation write-throughs an
    /// encrypted op to the CRDT outbox (Item 5). `None` keeps pure
    /// storage behavior (read paths, offline-locked contexts, tests).
    pub fn new(repos: &'a Repos, identity: Option<&'a Identity>) -> Self {
        Self { repos, identity }
    }

    // ------------------------------------------------------------------
    // Activation — the Stackelberg gate (US-3).
    // ------------------------------------------------------------------

    /// Activates the next runnable directive if (and only if) no
    /// directive is currently active. Exactly one directive is
    /// interactable at any moment.
    pub fn activate_next(&self, date: &str) -> Result<EngineOutcome, EngineError> {
        if let Some(active) = self.repos.active_directive()? {
            let estimated_minutes = self.current_phase_minutes(&active)?;
            let phase = self.phase_view(&active);
            return Ok(EngineOutcome::DirectiveActive {
                directive_id: active.id,
                milestone_id: active.milestone_id,
                phase,
                estimated_minutes,
            });
        }
        let Some(next) = self.repos.next_runnable_directive(date)? else {
            return Ok(EngineOutcome::Idle);
        };
        let estimated_minutes = self.current_phase_minutes(&next)?;
        self.repos
            .set_directive_state(&next.id, DirectiveState::Active, self.identity)?;
        self.ensure_milestone_active(&next.milestone_id)?;
        let phase = self.phase_view(&next);
        Ok(EngineOutcome::DirectiveActive {
            directive_id: next.id.clone(),
            milestone_id: next.milestone_id.clone(),
            phase,
            estimated_minutes,
        })
    }

    /// Snapshot of the currently active directive for HUD rendering.
    pub fn current(&self, date: &str) -> Result<EngineOutcome, EngineError> {
        self.activate_next(date) // idempotent if already active
    }

    // ------------------------------------------------------------------
    // Completion — ⌘+Enter path.
    // ------------------------------------------------------------------

    /// Marks the active directive (or its current progressive phase)
    /// complete. For progressive directives, intermediate completions
    /// advance the phase instead of closing the directive (§5.2). The
    /// reported minutes always come from the POST-advance row (E1), and
    /// the final phase row is marked done before the directive closes
    /// (E2) so peers never replicate a junk active-final-phase state.
    pub fn complete(&self, date: &str) -> Result<EngineOutcome, EngineError> {
        let Some(d) = self.repos.active_directive()? else {
            return self.activate_next(date);
        };
        if d.uses_progressive_activation() && d.progressive_step < d.progressive_total {
            self.repos.advance_progressive_step(&d.id, self.identity)?;
            // Re-read: the pre-advance snapshot still points at the old
            // step, so its minutes belong to the phase just finished.
            let fresh = self
                .repos
                .directive(&d.id)?
                .ok_or_else(|| StoreError::NotFound(format!("directive {}", d.id)))?;
            let estimated_minutes = self.current_phase_minutes(&fresh)?;
            let phase = self.phase_view(&fresh);
            return Ok(EngineOutcome::DirectiveActive {
                directive_id: fresh.id,
                milestone_id: fresh.milestone_id,
                phase,
                estimated_minutes,
            });
        }
        if d.uses_progressive_activation() {
            // Final phase: close the phase row first (replicated via
            // write-through inside advance), then the directive itself.
            self.repos.advance_progressive_step(&d.id, self.identity)?;
        }
        self.repos
            .set_directive_state(&d.id, DirectiveState::Completed, self.identity)?;
        self.maybe_complete_milestone(&d.milestone_id)?;
        Ok(EngineOutcome::DirectiveCompleted { directive_id: d.id })
    }

    // ------------------------------------------------------------------
    // Escape hatch — Esc path with mandatory categorization (§5.3).
    // ------------------------------------------------------------------

    /// Records a bailout with a mandatory reason, parks the directive,
    /// and reacts per the reason:
    ///
    /// * `external_dependency` → directive `blocked` (dormant), advance
    ///   to next unblocked thread;
    /// * `miscalculated_scope` → scope downsized (estimate halved,
    ///   floor 15 min) and re-queued for tomorrow;
    /// * `energy_depletion` → directive `skipped` (velocity event,
    ///   NOT a failure), low-cognitive recovery offered.
    pub fn bail_out(
        &self,
        date: &str,
        reason: BailoutReason,
        note: Option<&str>,
    ) -> Result<EngineOutcome, EngineError> {
        if crate::domain::check_date("date", date).is_err() {
            return Err(StoreError::Invalid("bad date".into()).into());
        }
        let Some(d) = self.repos.active_directive()? else {
            return self.activate_next(date);
        };
        if reason == BailoutReason::MiscalculatedScope && date_plus_days(date, 1).is_none() {
            return Err(StoreError::Invalid("bad date".into()).into());
        }
        self.repos
            .record_bailout(&d.id, reason, note, self.identity)?;
        let recovery = match reason {
            BailoutReason::ExternalDependency => {
                self.repos
                    .set_directive_state(&d.id, DirectiveState::Blocked, self.identity)?;
                // Advance to the next unblocked queued thread.
                let next = self.repos.next_runnable_directive(date)?;
                Some(RecoveryAction::AdvanceUnblocked {
                    next_directive_id: next.map(|n| n.id).unwrap_or_default(),
                })
            }
            BailoutReason::MiscalculatedScope => {
                // Downsize: halve the estimate (floor 15 min) and
                // requeue for tomorrow.
                let new_minutes = (d.estimated_minutes / 2).max(15);
                self.downsize_directive(&d, new_minutes, date)?;
                Some(RecoveryAction::DownsizeAndRequeue {
                    directive_id: d.id.clone(),
                    new_minutes,
                })
            }
            BailoutReason::EnergyDepletion => {
                self.repos
                    .set_directive_state(&d.id, DirectiveState::Skipped, self.identity)?;
                Some(RecoveryAction::LowCognitiveTask {
                    directive_id: d.id.clone(),
                })
            }
        };
        Ok(EngineOutcome::BailedOut {
            directive_id: d.id,
            reason,
            recovery,
        })
    }

    /// Halves-estimate requeue used by scope downsizing.
    fn downsize_directive(
        &self,
        d: &Directive,
        new_minutes: i64,
        today: &str,
    ) -> Result<(), EngineError> {
        let tomorrow = date_plus_days(today, 1).ok_or(StoreError::Invalid("bad date".into()))?;
        self.repos
            .reschedule_directive(&d.id, new_minutes, &tomorrow, self.identity)?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Check-in — evening 30-second audit (§5.4).
    // ------------------------------------------------------------------

    pub fn check_in(
        &self,
        date: &str,
        outcome: CheckInOutcome,
        note: Option<&str>,
    ) -> Result<EngineOutcome, EngineError> {
        if crate::domain::check_date("date", date).is_err() {
            return Err(StoreError::Invalid("bad date".into()).into());
        }
        self.repos
            .upsert_check_in(date, outcome, note, self.identity)?;
        Ok(EngineOutcome::CheckedIn {
            date: date.into(),
            outcome,
        })
    }

    // ----------------------------------------------------------------    // Internal helpers
    // ------------------------------------------------------------------

    fn phase_view(&self, d: &Directive) -> Option<(i64, i64)> {
        if d.uses_progressive_activation() {
            Some((d.progressive_step, d.progressive_total))
        } else {
            None
        }
    }

    /// Estimated minutes of the current execution unit.
    ///
    /// Self-heals a progressive directive whose phase rows went missing
    /// (CORE-6). This used to return
    /// `Invalid("current phase missing")`, which broke `activate_next`,
    /// `complete` and the HUD with no way forward — the directive could
    /// neither be run nor requeued. `ensure_phases` rebuilds the rows
    /// deterministically from the directive's own estimate and
    /// replicates the repair, so the only remaining failure is a
    /// directive genuinely too short to split, whose error now names
    /// the remedy.
    fn current_phase_minutes(&self, d: &Directive) -> Result<i64, EngineError> {
        if !d.uses_progressive_activation() {
            return Ok(d.estimated_minutes);
        }
        let minutes = self
            .repos
            .lock_conn()
            .query_row(
                "SELECT minutes FROM directive_phases WHERE directive_id = ?1 AND step = ?2",
                rusqlite::params![d.id, d.progressive_step],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map_err(StoreError::Sqlite)?;
        if let Some(minutes) = minutes {
            return Ok(minutes);
        }
        let rebuilt = self.repos.ensure_phases(&d.id, self.identity)?;
        rebuilt
            .into_iter()
            .find(|p| p.step == d.progressive_step)
            .map(|p| p.minutes)
            .ok_or_else(|| {
                StoreError::Invalid(format!(
                    "directive {} is corrupt: no phase {} after rebuild — reschedule it",
                    d.id, d.progressive_step
                ))
                .into()
            })
    }

    fn ensure_milestone_active(&self, milestone_id: &str) -> Result<(), EngineError> {
        let m = self
            .repos
            .lock_conn()
            .query_row(
                "SELECT status FROM milestones WHERE id = ?1",
                [milestone_id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(StoreError::Sqlite)?
            .ok_or(EngineError::MilestoneMissing(milestone_id.into()))?;
        if m == "pending" {
            self.repos.set_milestone_status(
                milestone_id,
                MilestoneStatus::Active,
                self.identity,
            )?;
        }
        Ok(())
    }

    /// Milestone completes when no queued/active directives remain.
    fn maybe_complete_milestone(&self, milestone_id: &str) -> Result<(), EngineError> {
        let remaining: i64 = self
            .repos
            .lock_conn()
            .query_row(
                "SELECT COUNT(*) FROM directives WHERE milestone_id = ?1
                 AND state IN ('queued', 'active', 'blocked')",
                [milestone_id],
                |r| r.get(0),
            )
            .map_err(StoreError::Sqlite)?;
        if remaining == 0 {
            self.repos.set_milestone_status(
                milestone_id,
                MilestoneStatus::Completed,
                self.identity,
            )?;
        }
        Ok(())
    }
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests;
