//! Velocity recalibration (PRD §5.4): skips are objective velocity
//! adjustments. Target velocity = Δ remaining milestones / Δ remaining
//! days. Future estimates shrink or grow via EWMA on completion ratio.

use crate::store::repo::Repos;

use super::EngineError;

/// Velocity snapshot for HUD / Tier-2 context.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Velocity {
    /// Milestones remaining.
    pub milestones_remaining: i64,
    /// Calendar days remaining to target date (>= 0).
    pub days_remaining: i64,
    /// Target velocity: milestones per day.
    pub target_per_day: f64,
    /// Observed completion ratio over the recent window (0.0..=1.0).
    pub completion_ratio: f64,
    /// Adjustment factor applied to future estimates (<1 = easier).
    pub estimate_adjustment: f64,
}

/// EWMA alpha for estimate recalibration: recent days matter more.
const EWMA_ALPHA: f64 = 0.4;

/// History window for the completion ratio (past 14 check-ins).
const WINDOW: i64 = 14;

pub fn compute(repos: &Repos, today: &str) -> Result<Option<Velocity>, EngineError> {
    let Some(goal) = repos.active_goal()? else {
        return Ok(None);
    };
    let milestones = repos.milestones_for_goal(&goal.id)?;
    let remaining = milestones
        .iter()
        .filter(|m| m.status == crate::domain::MilestoneStatus::Pending)
        .count() as i64;

    let days_remaining = goal
        .target_date
        .as_deref()
        .and_then(|td| crate::domain::days_between(td, today))
        .unwrap_or(0)
        .max(0);

    let target_per_day = if days_remaining > 0 {
        remaining as f64 / days_remaining as f64
    } else {
        0.0
    };

    // Completion ratio from recent check-ins (weighted: done=1, partial=0.5).
    let recent = repos.recent_check_ins(WINDOW)?;
    let ratio = if recent.is_empty() {
        1.0
    } else {
        let weights: Vec<f64> = (0..recent.len())
            .map(|i| EWMA_ALPHA * (1.0 - EWMA_ALPHA).powi(i as i32))
            .collect();
        let wsum: f64 = weights.iter().sum();
        let mut acc = 0.0;
        for (chk, w) in recent.iter().zip(weights) {
            let v = match chk.outcome {
                crate::domain::CheckInOutcome::Done => 1.0,
                crate::domain::CheckInOutcome::Partial => 0.5,
                crate::domain::CheckInOutcome::Skipped => 0.0,
            };
            acc += v * w;
        }
        (acc / wsum).clamp(0.0, 1.0)
    };

    // Estimate adjustment: below-target completion ⇒ downscale future
    // estimates to avoid compounding backlogs (zero-guilt: it's a
    // route recalculation, not a penalty). Floor at 60% of original.
    let estimate_adjustment = (0.6 + 0.4 * ratio).clamp(0.6, 1.0);

    Ok(Some(Velocity {
        milestones_remaining: remaining,
        days_remaining,
        target_per_day,
        completion_ratio: ratio,
        estimate_adjustment,
    }))
}

#[cfg(test)]
mod tests {
    use crate::store::open_in_memory;
    use crate::store::repo::Repos;

    use super::*;
    use crate::domain::{CheckInOutcome, MilestoneStatus};

    /// Fresh repo with NO goal (unlike engine::tests::setup_full).
    fn fresh() -> Repos {
        Repos::new(open_in_memory().unwrap(), 1)
    }

    #[test]
    fn velocity_no_goal_is_none() {
        let r = fresh();
        assert!(compute(&r, "2026-09-13").unwrap().is_none());
    }

    #[test]
    fn target_velocity_formula() {
        let r = fresh();
        let g = r
            .create_goal("Ship", None, Some("2026-09-23"), None)
            .unwrap(); // +10 days
        r.create_milestone(&g.id, "M1", None, 0, None).unwrap();
        r.create_milestone(&g.id, "M2", None, 1, None).unwrap();
        r.create_milestone(&g.id, "M3", None, 2, None).unwrap();
        r.set_milestone_status(
            &r.next_pending_milestone(&g.id).unwrap().unwrap().id,
            MilestoneStatus::Completed,
            None,
        )
        .unwrap();
        let v = compute(&r, "2026-09-13").unwrap().unwrap();
        assert_eq!(v.milestones_remaining, 2);
        assert_eq!(v.days_remaining, 10);
        assert!((v.target_per_day - 0.2).abs() < 1e-9);
    }

    #[test]
    fn skipped_days_shrink_future_estimates_never_punish() {
        let r = fresh();
        let g = r
            .create_goal("Ship", None, Some("2026-10-13"), None)
            .unwrap();
        r.create_milestone(&g.id, "M1", None, 0, None).unwrap();
        // 3 skipped days in a row.
        for d in ["2026-09-10", "2026-09-11", "2026-09-12"] {
            r.upsert_check_in(d, CheckInOutcome::Skipped, None, None)
                .unwrap();
        }
        let v = compute(&r, "2026-09-13").unwrap().unwrap();
        assert_eq!(v.completion_ratio, 0.0);
        // Adjustment hits the 60% floor — still present, never zero.
        assert!((v.estimate_adjustment - 0.6).abs() < 1e-9);
    }

    #[test]
    fn done_days_keep_full_pace() {
        let r = fresh();
        let g = r
            .create_goal("Ship", None, Some("2026-10-13"), None)
            .unwrap();
        r.create_milestone(&g.id, "M1", None, 0, None).unwrap();
        for d in ["2026-09-10", "2026-09-11", "2026-09-12"] {
            r.upsert_check_in(d, CheckInOutcome::Done, None, None)
                .unwrap();
        }
        let v = compute(&r, "2026-09-13").unwrap().unwrap();
        assert!((v.completion_ratio - 1.0).abs() < 1e-9);
        assert!((v.estimate_adjustment - 1.0).abs() < 1e-9);
    }

    #[test]
    fn partial_counts_half() {
        let r = fresh();
        let g = r
            .create_goal("Ship", None, Some("2026-10-13"), None)
            .unwrap();
        r.create_milestone(&g.id, "M1", None, 0, None).unwrap();
        r.upsert_check_in("2026-09-12", CheckInOutcome::Partial, None, None)
            .unwrap();
        let v = compute(&r, "2026-09-13").unwrap().unwrap();
        assert_eq!(v.completion_ratio, 0.5);
        assert!((v.estimate_adjustment - 0.8).abs() < 1e-9);
    }
}
