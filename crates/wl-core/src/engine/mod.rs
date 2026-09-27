//! Tier-3 Local Heuristic Engine (PRD §4): the offline Rust-native
//! brain. Owns the single-active-directive invariant (US-3),
//! progressive phase activation (§5.2), the frictionful escape hatch
//! state machine (§5.3), evening check-ins (§5.4), and velocity
//! recalibration.
//!
//! Pure synchronous logic over [`Repos`] — no timers, no async; the
//! Tauri shell drives wall-clock events.

pub mod calibration;
pub mod velocity;

use crate::crypto::identity::Identity;
use crate::domain::*;
use crate::store::repo::Repos;
use crate::store::StoreError;

/// The most steps a progressive directive is allowed to have.
///
/// Mirrors the planner's own ceiling (`2..=4`, PRD §5.2) with a little
/// headroom, and exists so the phase-closing loop in `mark_complete` has a
/// bound that does not come from a replicated integer. `progressive_total`
/// arrives from a sealed payload with no cross-field check, so a step past
/// the total is possible in principle and a loop that trusted it would be
/// a spin on the ledger's one write.
const MAX_PROGRESSIVE_STEPS: i64 = 8;

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
    /// Directive fully completed → next one unlocks. Carries the id
    /// whether completion came from the canvas or from the ledger, so a
    /// caller that re-reads the canvas does not have to know which.
    DirectiveCompleted { directive_id: String },
    /// Queue empty for today — calm idle state (never a backlog view).
    Idle,
    /// Check-in recorded.
    CheckedIn {
        date: String,
        outcome: CheckInOutcome,
    },
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
        // Compute the phase view and validate the milestone BEFORE the
        // write. The order matters: `set_directive_state(.., Active)`
        // commits, and `ensure_milestone_active` can then fail with
        // `MilestoneMissing` — leaving an active directive whose
        // milestone was never promoted, with the caller holding an Err
        // and no idea the activation landed.
        let estimated_minutes = self.current_phase_minutes(&next)?;
        self.ensure_milestone_active(&next.milestone_id)?;
        self.repos
            .set_directive_state(&next.id, DirectiveState::Active, self.identity)?;
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

    /// ⌘+Enter with nothing active is `Idle`, not an implicit activation,
    /// so `date` is no longer consulted here. The parameter stays so the
    /// command surface and the canvas call site keep their shape; the
    /// canvas activates via [`Engine::current`] on load.
    pub fn complete(&self, _date: &str) -> Result<EngineOutcome, EngineError> {
        // ⌘+Enter on an idle canvas is `Idle`, not "start the next
        // thing". It used to fall through to `activate_next`, so a stray
        // keystroke on an empty canvas began a directive the user never
        // asked for.
        let Some(d) = self.repos.active_directive()? else {
            return Ok(EngineOutcome::Idle);
        };
        if d.uses_progressive_activation() && d.progressive_step < d.progressive_total {
            // Self-heal first. `advance_progressive_step` hard-fails on a
            // missing phase row, and only `activate_next` ran the repair
            // — so a progressive directive whose phase rows went missing
            // (a truncated pull) could never be completed: every ⌘+Enter
            // errored until the canvas happened to reload.
            self.current_phase_minutes(&d)?;
            // The return value is load-bearing. It is `false` when the
            // row moved under us (a concurrent sync merge), and the old
            // code discarded it with `?` — so the phase was marked done
            // and the caller was told the directive was still active at
            // `phase: (total, total)`, having advanced nothing.
            if self.repos.advance_progressive_step(&d.id, self.identity)? {
                // Re-read: the pre-advance snapshot still points at the
                // old step, so its minutes belong to the phase just
                // finished.
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
    // Marking a task done — the ledger's only write.
    // ------------------------------------------------------------------

    /// Marks an arbitrary directive `Completed`, from outside the canvas.
    ///
    /// The canvas is read-only (2026-09-27: the ⌘+Enter and Escape
    /// gestures were both removed, and the whole escape-hatch path with
    /// them). So this is the only way a task ever resolves, and it has to
    /// carry the entire state transition that `Engine::complete` used to
    /// do for the ACTIVE directive:
    ///
    /// * **Every remaining phase is closed**, not just the current one. A
    ///   tick is a statement about the task, and the ledger is task-level;
    ///   advancing one phase per tap would mean four taps to say "done",
    ///   which is a different control wearing the same clothes. Each step
    ///   goes through `advance_progressive_step`, so HLC and the outbox
    ///   are stamped exactly as they are on the canvas path.
    /// * **The milestone is re-evaluated.** `maybe_complete_milestone`
    ///   counts `blocked` as outstanding, so a milestone cannot be
    ///   reported done with unreachable work inside it.
    /// * **The Stackelberg invariant is left to the caller** to advance,
    ///   exactly as `complete_directive` does: if the marked directive was
    ///   the active one, the command then calls `Engine::current`, which
    ///   activates the next thing or reports `Idle`. Duplicating that here
    ///   would mean two places that know how to advance the canvas.
    ///
    /// Marking an already-`completed` directive is an idempotent no-op
    /// rather than an error: the ledger's control is a tick, and a second
    /// tap on a ticked row is a person confirming, not a mistake.
    pub fn mark_complete(
        &self,
        date: &str,
        directive_id: &str,
    ) -> Result<EngineOutcome, EngineError> {
        crate::domain::check_date("date", date).map_err(StoreError::Invalid)?;
        let Some(d) = self.repos.directive(directive_id)? else {
            return Err(StoreError::NotFound(format!("directive {directive_id}")).into());
        };
        if d.state == DirectiveState::Completed {
            return Ok(EngineOutcome::DirectiveCompleted { directive_id: d.id });
        }
        // Everything phase-related is inside this branch. It used to be a
        // bare `while` afterwards, which sent a MONOLITHIC task — no phase
        // rows at all — through `advance_progressive_step` and failed with
        // "current or next phase missing". Ticking a 20-minute task is the
        // single most common thing the ledger can be asked to do.
        if d.uses_progressive_activation() {
            // Self-heal first, for the same reason `complete` does: a
            // progressive directive whose phase rows went missing would
            // make every `advance_progressive_step` fail, and the ledger
            // would be the one surface that cannot tick a task off.
            self.current_phase_minutes(&d)?;
            // Advance until the LAST step is the current one, then close it.
            // Two steps, not one loop: `advance_progressive_step` marks the
            // step it leaves, so a loop alone ends with the final phase
            // still `active` while the directive reads `completed`.
            // `complete` has the same two-part shape for the same reason.
            //
            // The bound is explicit rather than reading `d.progressive_step`
            // in the condition, because that field is NOT mutated in the
            // body: the loop is driven by the store returning `true`, and a
            // `progressive_total` a peer inflated would otherwise make
            // "cannot spin" an argument rather than a fact.
            for _ in 0..d.progressive_total.clamp(0, MAX_PROGRESSIVE_STEPS) {
                // `false` means the row moved under us (a concurrent sync
                // merge). Stop rather than spin; the directive still gets
                // marked complete, which is what the user asked for.
                if !self.repos.advance_progressive_step(&d.id, self.identity)? {
                    break;
                }
            }
            self.repos.advance_progressive_step(&d.id, self.identity)?;
        }
        self.repos
            .set_directive_state(&d.id, DirectiveState::Completed, self.identity)?;
        self.maybe_complete_milestone(&d.milestone_id)?;
        Ok(EngineOutcome::DirectiveCompleted { directive_id: d.id })
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
        // A step beyond the total is a replicated-op artefact, not a
        // corrupt user plan: `apply_upsert` writes `progressive_step`
        // and `progressive_total` straight from the payload with no
        // cross-field check, so a peer (or a half-applied merge) can set
        // `step = 9, total = 2`. The rebuild only makes steps 1..=2, so
        // the old error fired on every canvas load and no engine path
        // could ever requeue the directive. Reset to the first phase —
        // the cheapest state that is inside the directive's own plan.
        if d.progressive_step > d.progressive_total {
            self.repos.reset_progressive_step(&d.id, self.identity)?;
        }
        rebuilt
            .into_iter()
            .find(|p| p.step == 1)
            .map(|p| p.minutes)
            .ok_or_else(|| {
                StoreError::Invalid(format!(
                    "directive {} is corrupt: no phase after rebuild — reschedule it",
                    d.id
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
        // Reopen a `completed` or `demoted` milestone too. Only
        // `pending` used to be promoted, so a directive created against
        // a finished milestone (by a peer, or by re-planning) ran with
        // the milestone still marked done: `velocity` stopped counting it
        // as remaining, and `maybe_complete_milestone` re-completed it as
        // a no-op. A milestone with live work under it is `active`,
        // whatever it was before.
        if m != "active" {
            self.repos.set_milestone_status(
                milestone_id,
                MilestoneStatus::Active,
                self.identity,
            )?;
        }
        Ok(())
    }

    /// Milestone completes when no outstanding directives remain.
    ///
    /// `skipped` counts as OUTSTANDING. It used to be left out of the
    /// set, so a milestone with two directives — one bailed out for
    /// energy (→ `skipped`) and one completed — was marked `completed`
    /// with the skipped directive still unstarted and unreachable:
    /// `next_runnable_directive` only selects `queued`, and nothing
    /// requeues a skip. The milestone was then reported as done while
    /// real work sat inside it.
    fn maybe_complete_milestone(&self, milestone_id: &str) -> Result<(), EngineError> {
        let remaining: i64 = self
            .repos
            .lock_conn()
            .query_row(
                "SELECT COUNT(*) FROM directives WHERE milestone_id = ?1
                 AND state IN ('queued', 'active', 'blocked', 'skipped')",
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
