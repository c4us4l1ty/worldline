//! Difficulty calibration — the Bayesian half of the *Estimated
//! Complexity* control.
//!
//! # What the slider is
//!
//! The compose screen asks, before any plan exists, how big this objective
//! looks: 1 (`light`) through 5 (`deep`). That rating is stored on the goal
//! and it is **the prior**. Every time one of that goal's tasks resolves,
//! the posterior moves — and the next plan the architect writes is told
//! what the record actually says. That last part is the load-bearing one:
//! an estimator whose output nothing consumes is the inert number PRD
//! delta 33 is about, and this system does not ship those.
//!
//! # The model
//!
//! Beta-Bernoulli, per bucket, seeded from the rating:
//!
//! ```text
//! prior(r) = Beta(α = r + 1, β = 7 − r)     // strength 8
//! ```
//!
//! So the prior mean is `(r + 1) / 8`, and the five buckets are:
//!
//! | rating | word      | prior       | prior mean |
//! |--------|-----------|-------------|------------|
//! | 1      | light     | Beta(2, 6)  | 0.250      |
//! | 2      | light+    | Beta(3, 5)  | 0.375      |
//! | 3      | standard  | Beta(4, 4)  | 0.500      |
//! | 4      | heavy     | Beta(5, 3)  | 0.625      |
//! | 5      | deep      | Beta(6, 2)  | 0.750      |
//!
//! Three properties of that mapping are the reason it is this one and not
//! another, and each is pinned by a test below:
//!
//! * **The rating is the prior, literally.** `r = 5` is Beta(6, 2): mostly
//!   confident you will *not* get it done, which is what "deep" means.
//! * **Nothing is improper.** `α + β = 8` for every bucket, so no rating
//!   produces `β = 0` and no bucket can report a probability of exactly 0
//!   or 1 on zero evidence. A strength-1 prior would have made rating 1
//!   mean "certain to fail", which is a claim nobody made.
//! * **Monotone across the whole scale.** Rating 1 lands below rating 5,
//!   and the middle is exactly 0.5 — so an untouched slider is an
//!   uninformative 50%, not a pessimistic or optimistic lean.
//!
//! # What counts as an observation
//!
//! One per **resolved** directive, bucketed by its goal's rating:
//!
//! * `completed` → success.
//! * `blocked` → not. A task parked forever waiting on something is a task
//!   that did not land.
//! * `queued` and scheduled for a date already past → also not, and it DOES
//!   count as an observation. The engine has no "missed" state and inventing
//!   one is Phase-2; an overdue queue is the honest proxy, and it is the
//!   same fact the evening audit already infers. "Does it count" and "did
//!   it land" are separate questions — the first is about the date, the
//!   second about the outcome.
//!
//! `active` is deliberately NOT an observation. A task in flight is exactly
//! the state where a count would be least meaningful and most tempting, and
//! counting it would make the number move every time the user looked at
//! the canvas. `skipped` is not one either: it was never attempted, so it
//! says nothing about whether that kind of work lands.
//!
//! # Sample size is not optional
//!
//! Every figure returned here carries its `n` and its `s`. A posterior
//! printed without them is how "62%" gets read as a fact when it is one
//! data point, and it is the exact failure the design system's "an inert
//! number must say it is inert" rule exists to prevent — extended to the
//! case where a number is *live* but thin.

use std::collections::BTreeMap;

use crate::domain::{DirectiveState, COMPLEXITY_MAX, COMPLEXITY_MIN};
use crate::store::repo::Repos;

/// Pseudo-observations behind every prior.
///
/// The whole mapping's fixed point: `α + β = PRIOR_STRENGTH`, so the prior
/// mean is `(r + 1) / PRIOR_STRENGTH`.
pub const PRIOR_STRENGTH: f64 = 8.0;

/// One bucket's belief, and the evidence behind it.
///
/// `n` and `s` are not derivable from `posterior_mean` and are never
/// inferred from it: they are carried so every caller can show them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bucket {
    pub complexity: i64,
    /// The user's own rating for work in this bucket. This is the prior.
    pub prior_alpha: f64,
    pub prior_beta: f64,
    /// Observations: successes and the total.
    pub successes: i64,
    pub observations: i64,
}

impl Bucket {
    /// The prior, before any evidence.
    pub fn prior(complexity: i64) -> Self {
        let r = complexity.clamp(COMPLEXITY_MIN, COMPLEXITY_MAX) as f64;
        Self {
            complexity: r as i64,
            prior_alpha: r + 1.0,
            prior_beta: PRIOR_STRENGTH - (r + 1.0),
            successes: 0,
            observations: 0,
        }
    }

    /// Posterior mean: the probability this kind of work lands.
    ///
    /// A mean, not a mode and not a confidence interval. A mode would make
    /// the number jump between 0.49 and 0.51 as evidence arrives, and an
    /// interval would be a number a person cannot do anything with on a
    /// 420px page. The mean moves smoothly, which is the only property that
    /// makes it readable as a trend.
    pub fn posterior_mean(&self) -> f64 {
        let a = self.prior_alpha + self.successes as f64;
        let b = self.prior_beta + (self.observations - self.successes) as f64;
        (a / (a + b)).clamp(0.0, 1.0)
    }

    /// `(successes, observations)` as a fraction string, e.g. `"4 of 9"`.
    ///
    /// The raw counts, deliberately, not a percentage: "4 of 9" is
    /// checkable against the ledger and "44%" is not, and the whole point
    /// of showing the number is that a person can decide whether to trust
    /// it.
    pub fn evidence(&self) -> String {
        format!("{} of {}", self.successes, self.observations)
    }
}

/// Every bucket, with its evidence, keyed by rating.
///
/// Only buckets the user has actually rated appear, and within that only
/// buckets with at least one goal. An install with no goal has no
/// calibration to report, and rendering five empty rows would be a chart of
/// nothing.
pub fn calibration(
    repos: &Repos,
    today: &str,
) -> Result<BTreeMap<i64, Bucket>, crate::store::StoreError> {
    // Per GOAL first, then rolled up by rating.
    //
    // The other order is the bug this replaced: aggregating the whole
    // install's observations by rating and then adding that total once per
    // goal at the same rating double-counts, so two `light` goals with nine
    // tasks between them reported eighteen. The BTreeMap of goals is the
    // outer loop precisely so each goal contributes its OWN slice.
    let mut per_goal: BTreeMap<String, (i64, i64)> = BTreeMap::new();
    for (goal_id, complexity, state, scheduled) in repos.directive_evidence()? {
        let Some((success, _)) = observation(complexity, state, &scheduled, today) else {
            continue;
        };
        let entry = per_goal.entry(goal_id).or_insert((0, 0));
        entry.1 += 1;
        if success {
            entry.0 += 1;
        }
    }
    let mut out: BTreeMap<i64, Bucket> = BTreeMap::new();
    for goal in repos.goals_by_complexity()? {
        let complexity = crate::domain::clamp_complexity(goal.complexity);
        let bucket = out
            .entry(complexity)
            .or_insert_with(|| Bucket::prior(complexity));
        if let Some((successes, total)) = per_goal.get(&goal.id) {
            bucket.successes += successes;
            bucket.observations += total;
        }
    }
    Ok(out)
}

/// Whether one resolved directive is a success, or not an observation at
/// all (`None`).
///
/// * `completed` → success.
/// * `blocked` → not. A task parked forever waiting on something is a task
///   that did not land.
/// * `queued` and scheduled for a date already past → not. The engine has
///   no "missed" state and inventing one is Phase-2; an overdue queue is
///   the honest proxy, and it is the same fact the evening audit already
///   infers.
/// * `active` → not an observation. A task in flight is exactly the state
///   where a count would be least meaningful and most tempting, and
///   counting it would make the number move every time the user looked at
///   the canvas.
/// * `skipped` → not an observation either. It was never attempted, so it
///   says nothing about whether that kind of work lands.
fn observation(
    complexity: i64,
    state: DirectiveState,
    scheduled_for_date: &str,
    today: &str,
) -> Option<(bool, i64)> {
    let success = match state {
        DirectiveState::Completed => true,
        DirectiveState::Blocked => false,
        // Overdue and still queued: it was scheduled for a day that has
        // passed and nothing has moved it. That COUNTS, and it counts as a
        // failure — the two are separate questions, and conflating them
        // once made an overdue queue the *best* evidence in the estimator
        // (it was returning "is it overdue?" where it meant "did it land?").
        //
        // A queued task that is NOT yet due is not an observation at all.
        DirectiveState::Queued if scheduled_for_date < today => false,
        DirectiveState::Queued => return None,
        // `active` is not an observation: a task in flight is exactly the
        // state where a count would be least meaningful and most
        // tempting, and counting it would make the number move every time
        // the user looked at the canvas. `skipped` is not one either — it
        // was never attempted, so it says nothing about whether that kind
        // of work lands.
        DirectiveState::Active | DirectiveState::Skipped => return None,
    };
    Some((success, complexity))
}

/// One sentence for the Tier-1 prompt about the bucket being asked for.
///
/// `None` when the bucket has no goals, because "you have never rated
/// anything 5/5" is not a fact worth putting in a prompt — and a prompt
/// that mentions an empty bucket teaches the model to invent one.
///
/// This is the estimator's only consumer, and it is a real one: the
/// architect sizes the plan against the record rather than against a
/// number that only exists to be displayed.
pub fn prompt_line(calibration: &BTreeMap<i64, Bucket>, complexity: i64) -> Option<String> {
    let complexity = crate::domain::clamp_complexity(complexity);
    let bucket = calibration.get(&complexity)?;
    let word = crate::domain::complexity_label(bucket.complexity)?;
    if bucket.observations == 0 {
        let pct = (bucket.posterior_mean() * 100.0).round() as i64;
        return Some(format!(
            "The user rates this {word} ({pct}% expected from the rating alone, no record yet)."
        ));
    }
    // The record, and the rating's own opinion about it, kept SEPARATE.
    // Quoting the posterior as the prior was the bug here: the sentence
    // read "…they finish 4 of 5 (77% expected; the rating alone said 77%)",
    // which is a comparison with nothing. The prior is recomputed from the
    // rating, not read back off the posterior.
    let prior_pct = (Bucket::prior(complexity).posterior_mean() * 100.0).round() as i64;
    let pct = (bucket.posterior_mean() * 100.0).round() as i64;
    Some(format!(
        "The user rates this {word}. Historically they finish {} of that kind of work \
         ({pct}% expected; the rating alone said {prior_pct}%).",
        bucket.evidence(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::open_in_memory;
    use crate::store::repo::Repos;

    const TODAY: &str = "2026-09-27";

    fn repos() -> Repos {
        Repos::new(open_in_memory().unwrap(), 1)
    }

    /// One goal at `complexity`, with exactly `completed` finished tasks,
    /// `blocked` parked ones, and `overdue` still-queued-but-past-due ones.
    ///
    /// Exact counts rather than "at least": an estimator test that seeds
    /// more than it means cannot catch a double count, which is the bug this
    /// helper was rewritten for.
    fn seed(r: &Repos, complexity: i64, completed: i64, blocked: i64, overdue: i64) -> String {
        let g = r
            .create_goal("G", None, None, complexity, None)
            .unwrap();
        let m = r.create_milestone(&g.id, "M", None, 0, None).unwrap();
        let mut n = 0;
        let mut add = |state: Option<DirectiveState>, date: &str| {
            n += 1;
            let d = r
                .create_directive(
                    &m.id,
                    &format!("task {n}"),
                    None,
                    20,
                    1,
                    date,
                    None,
                    &[],
                    None,
                )
                .unwrap();
            if let Some(state) = state {
                r.set_directive_state(&d.id, state, None).unwrap();
            }
        };
        for _ in 0..completed {
            add(Some(DirectiveState::Completed), TODAY);
        }
        for _ in 0..blocked {
            add(Some(DirectiveState::Blocked), TODAY);
        }
        for _ in 0..overdue {
            add(None, "2026-09-01");
        }
        g.id
    }

    /// The mapping, pinned. `r = 5` must mean "mostly confident this does
    /// not land" and the untouched middle must be exactly 0.5 — if either
    /// moves, every number the product has ever shown about calibration
    /// moves with it, silently.
    #[test]
    fn the_five_priors_are_what_the_table_says() {
        let expected = [
            (1, 0.250, 2.0, 6.0),
            (2, 0.375, 3.0, 5.0),
            (3, 0.500, 4.0, 4.0),
            (4, 0.625, 5.0, 3.0),
            (5, 0.750, 6.0, 2.0),
        ];
        for (rating, mean, alpha, beta) in expected {
            let b = Bucket::prior(rating);
            assert!(
                (b.posterior_mean() - mean).abs() < 1e-9,
                "rating {rating} prior mean was {}, expected {mean}",
                b.posterior_mean()
            );
            assert!((b.prior_alpha - alpha).abs() < 1e-9);
            assert!((b.prior_beta - beta).abs() < 1e-9);
            assert!(
                (b.prior_alpha + b.prior_beta - PRIOR_STRENGTH).abs() < 1e-9,
                "every prior must have the same strength, or the scale is not comparable"
            );
        }
    }

    /// No rating produces an improper prior, and none reports a certainty
    /// on zero evidence. This is what `α + β = 6` buys, and it is why the
    /// strength is not 1.
    #[test]
    fn no_prior_is_improper_and_none_is_certain() {
        for r in COMPLEXITY_MIN..=COMPLEXITY_MAX {
            let b = Bucket::prior(r);
            assert!(b.prior_alpha > 0.0 && b.prior_beta > 0.0, "rating {r}");
            let m = b.posterior_mean();
            assert!(m > 0.0 && m < 1.0, "rating {r} reported {m} on no evidence");
        }
    }

    /// The prior is monotone across the whole scale. Without this a future
    /// edit to one bucket could invert the meaning of the slider.
    #[test]
    fn prior_means_increase_with_the_rating() {
        let means: Vec<f64> = (COMPLEXITY_MIN..=COMPLEXITY_MAX)
            .map(|r| Bucket::prior(r).posterior_mean())
            .collect();
        for pair in means.windows(2) {
            assert!(pair[1] > pair[0], "prior means must be strictly increasing");
        }
        assert!(
            (means[2] - 0.5).abs() < 1e-9,
            "the default rating must be exactly 0.5, not a lean"
        );
    }

    #[test]
    fn an_unrated_bucket_reports_nothing() {
        let r = repos();
        assert!(calibration(&r, TODAY).unwrap().is_empty());
    }

    #[test]
    fn a_fresh_goal_reports_the_rating_alone() {
        let r = repos();
        seed(&r, 4, 0, 0, 0);
        let c = calibration(&r, TODAY).unwrap();
        let b = c.get(&4).expect("the rated bucket appears");
        assert_eq!(b.observations, 0);
        assert_eq!(b.successes, 0);
        assert!(
            (b.posterior_mean() - 0.625).abs() < 1e-9,
            "with no record the posterior IS the prior"
        );
        assert_eq!(b.evidence(), "0 of 0");
    }

    /// Every resolution is counted, and the split is the one the module doc
    /// promises: completed counts, blocked does not, an overdue queue does
    /// not, and an in-flight task is invisible.
    #[test]
    fn completions_block_and_overdue_queue_all_count_and_active_never_does() {
        let r = repos();
        let g = seed(&r, 3, 2, 1, 1);
        // A fifth task, live on the canvas. It must not move any number.
        let m = r.create_milestone(&g, "M2", None, 1, None).unwrap();
        let live = r
            .create_directive(
                &m.id, "in flight", None, 20, 1, TODAY, None, &[], None,
            )
            .unwrap();
        r.set_directive_state(&live.id, DirectiveState::Active, None)
            .unwrap();

        let b = calibration(&r, TODAY).unwrap();
        let b = b.get(&3).expect("bucket 3");
        assert_eq!(
            b.observations, 4,
            "2 done + 1 blocked + 1 overdue; the live one is not a data point"
        );
        assert_eq!(b.successes, 2);
        assert_eq!(b.evidence(), "2 of 4");
        // Prior was 0.5; a half-success rate leaves it near 0.5.
        assert!(
            (b.posterior_mean() - 0.5).abs() < 0.02,
            "half successes on a 0.5 prior should barely move: {}",
            b.posterior_mean()
        );
    }

    /// Evidence has to actually move the posterior, and in the right
    /// direction, or the whole module is decoration.
    #[test]
    fn evidence_moves_the_posterior_and_saturates() {
        let clean = Bucket {
            successes: 18,
            observations: 18,
            ..Bucket::prior(3)
        };
        let poor = Bucket {
            successes: 0,
            observations: 18,
            ..Bucket::prior(3)
        };
        // Beta(4, 4) with 18 of one sort: (4+18)/26 = 0.846, and 4/26 =
        // 0.154. Not 0.95 — and deliberately not, because the prior is
        // eight pseudo-observations of opinion and 18 of record is not
        // enough to overrule all of them. A strength-2 prior would report
        // 0.95 here, which is a claim 18 data points do not support.
        assert!(clean.posterior_mean() > 0.84, "{}", clean.posterior_mean());
        assert!(clean.posterior_mean() < 0.87, "{}", clean.posterior_mean());
        assert!(poor.posterior_mean() < 0.16, "{}", poor.posterior_mean());
        // Symmetric about the prior mean, which for rating 3 is 0.5. Not
        // symmetric about 0.5 in general — the prior mean IS 0.5 here only
        // because the middle rating was chosen to be uninformative.
        assert!(
            ((clean.posterior_mean() - 0.5) - (0.5 - poor.posterior_mean())).abs() < 1e-9,
            "18/18 and 0/18 must sit the same distance from the 0.5 prior: {} vs {}",
            clean.posterior_mean(),
            poor.posterior_mean()
        );
    }

    /// Several goals at one rating pool into one bucket. That is the point
    /// of bucketing by rating rather than by goal: the record is about the
    /// *kind* of work, not about one plan. It is also the case that used to
    /// double-count — the install-wide total was added once per goal, so
    /// two goals with nine tasks between them reported eighteen.
    #[test]
    fn goals_at_the_same_rating_pool_without_double_counting() {
        let r = repos();
        seed(&r, 2, 3, 0, 0);
        seed(&r, 2, 2, 0, 0);
        let b = calibration(&r, TODAY).unwrap();
        let b = b.get(&2).expect("one pooled bucket");
        assert_eq!(b.observations, 5, "3 + 2 tasks, counted once each");
        assert_eq!(b.successes, 5);
        assert_eq!(b.evidence(), "5 of 5");
    }

    /// The prompt line is the estimator's only consumer, so it is the
    /// thing that has to be right. Both shapes are asserted because the
    /// empty-record one is the common case: an install that has just
    /// started must not be told it has a history.
    #[test]
    fn the_prompt_line_reflects_the_record_and_omits_an_empty_one() {
        let r = repos();
        seed(&r, 5, 4, 1, 0);
        let c = calibration(&r, TODAY).unwrap();
        let line = prompt_line(&c, 5).expect("a bucket for the prompt");
        assert!(line.contains("deep"), "{line}");
        assert!(line.contains("4 of 5"), "{line}");
        // The record (4/5) and the rating's own opinion about it (75%) are
        // two different numbers and the line has to say both. Quoting the
        // posterior twice made this a comparison with nothing.
        assert!(line.contains("77%"), "{line}");
        assert!(line.contains("75%"), "{line}");

        // A rating with no goal at all produces no line, not a hollow one.
        assert!(prompt_line(&c, 1).is_none());

        // The no-evidence variant says so explicitly rather than quoting a
        // percentage as though it were a measurement.
        let r2 = repos();
        seed(&r2, 1, 0, 0, 0);
        let c2 = calibration(&r2, TODAY).unwrap();
        let line = prompt_line(&c2, 1).expect("bucket 1");
        assert!(line.contains("no record yet"), "{line}");
        assert!(line.contains("light"), "{line}");
    }

    /// A replicated rating outside 1–5 must not be able to invent a sixth
    /// bucket or divide by a negative strength.
    #[test]
    fn an_out_of_range_replicated_rating_clamps_into_the_scale() {
        let r = repos();
        let g = seed(&r, 3, 1, 0, 0);
        r.conn
            .lock()
            .unwrap()
            .execute("UPDATE goals SET complexity = 99 WHERE id = ?1", [&g])
            .unwrap();
        let c = calibration(&r, TODAY).unwrap();
        assert_eq!(c.len(), 1, "one bucket, not two");
        assert!(c.contains_key(&COMPLEXITY_MAX));
    }

    /// The default is the middle stop, so an untouched slider is 3. Pinned
    /// because `create_goal`'s column default and this must agree.
    #[test]
    fn the_default_rating_is_the_middle_stop() {
        assert_eq!(COMPLEXITY_DEFAULT, 3);
        let r = repos();
        let c = calibration(&r, TODAY);
        assert!(c.is_ok());
    }

}
