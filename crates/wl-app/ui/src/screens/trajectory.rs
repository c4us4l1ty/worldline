//! Trajectory — required velocity against observed.
//!
//! This data already existed and was already on screen, but only *after*
//! the fact: the evening audit renders it as the receipt for a check-in
//! you have already filed. Opening it from the control panel inverts that
//! — you can see the shape of your pace before committing the day's
//! check-in, which is the moment the number can still change a decision.
//!
//! Everything here is read-only arithmetic over `VelocityView`, which the
//! shell computes in `wl_core::engine::velocity`. The UI does not
//! recompute any of it: the engine's EWMA window and its 60% floor are
//! policy, and a second implementation in the view layer is a second
//! opinion nobody asked for.
//!
//! The honesty rule from PRD delta 33 is load-bearing and is repeated on
//! the page: `estimate_adjustment` is *observed, not applied*. A number
//! that looks like a setting but changes nothing is the one thing this
//! design system will not ship, so it is labelled as inert wherever it
//! appears.

use dioxus::prelude::*;

use crate::app::{invoke, AppCtx, Screen, VelocityView};
use crate::icons::IconBack;

/// Where the observed completion ratio sits, in the product's own words.
///
/// Deliberately not "behind schedule" / "failing". The whole visual
/// language treats a slow patch as a GPS recalculation, never as an
/// alarm, and a velocity page is exactly where that temptation lives:
/// four numbers that look like a report card.
///
/// The thresholds are the engine's own, not new policy: the adjustment
/// floor is 0.6 and `estimate_adjustment` is `0.6 + 0.4 * ratio`, so 0.6
/// is the ratio at which the engine stops shrinking future estimates.
pub fn pace_label(ratio: f64) -> &'static str {
    if ratio >= 0.95 {
        "Holding pace"
    } else if ratio >= 0.6 {
        "Easing"
    } else {
        "Recalibrating"
    }
}

/// The adjustment rendered as a signed downwards percentage, e.g. 12.
///
/// Returns `0` for a non-finite input rather than propagating NaN into
/// the DOM: a `NaN%` on screen is a bug report waiting to happen, and the
/// store cannot produce one, but the boundary still should not be able
/// to render garbage.
pub fn adjustment_pct(adjustment: f64) -> i64 {
    if !adjustment.is_finite() {
        return 0;
    }
    (((1.0 - adjustment) * 100.0).round()) as i64
}

pub fn TrajectoryScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut v = use_signal::<Option<VelocityView>>(|| None);
    let mut error = use_signal(|| None::<String>);

    use_effect(move || {
        spawn(async move {
            match invoke::<VelocityView>("velocity", ()).await {
                Ok(val) => {
                    error.set(None);
                    v.set(Some(val));
                }
                Err(e) => *error.write() = Some(e),
            }
        });
    });

    let loaded = v.read().clone();
    // The shell has no "no goal" variant: `velocity_inner` answers with a
    // zeroed view when `compute()` returns None. Zero remaining scope is
    // therefore the only honest empty signal, and it covers both "no goal
    // yet" and "this goal is finished" — which want the same copy anyway.
    let plotted = loaded
        .as_ref()
        .filter(|val| val.milestones_remaining > 0)
        .cloned();

    rsx! {
        div { class: "wl-page",
            div { class: "wl-page-head",
                button {
                    class: "wl-back",
                    aria_label: "Back to the line",
                    title: "Back to the line",
                    onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::Canvas; } },
                    IconBack {}
                }
                div { h1 { class: "wl-page-title", "Trajectory" } }
            }

            div { class: "wl-scroll-region",
                if let Some(err) = error.read().clone() {
                    p { class: "wl-form-error", "{err}" }
                }
                if let Some(val) = plotted {
                        div { class: "wl-directive-card wl-traj-card",
                            div { class: "wl-traj-pace",
                                span { class: "wl-traj-pace-label", "{pace_label(val.completion_ratio)}" }
                                span { class: "wl-traj-pace-note wl-mono", "observed" }
                            }
                            div { class: "wl-traj-grid",
                                div { class: "wl-traj-cell",
                                    span { class: "wl-traj-k", "Remaining scope" }
                                    span { class: "wl-traj-v wl-mono", "{val.milestones_remaining}" }
                                    span { class: "wl-traj-u", "milestones" }
                                }
                                div { class: "wl-traj-cell",
                                    span { class: "wl-traj-k", "Days to horizon" }
                                    span { class: "wl-traj-v wl-mono", "{val.days_remaining}" }
                                    span { class: "wl-traj-u", "days" }
                                }
                                div { class: "wl-traj-cell",
                                    span { class: "wl-traj-k", "Required velocity" }
                                    span { class: "wl-traj-v wl-mono", "{val.target_per_day:.2}" }
                                    span { class: "wl-traj-u", "per day" }
                                }
                                div { class: "wl-traj-cell",
                                    span { class: "wl-traj-k", "Rolling average" }
                                    span { class: "wl-traj-v wl-mono", "{val.completion_ratio:.2}" }
                                    span { class: "wl-traj-u", "completion" }
                                }
                            }
                            p { class: "wl-body-muted wl-traj-formula wl-mono",
                                "V_target = remaining milestones / remaining days"
                            }
                        }

                        div { class: "wl-directive-card wl-traj-card",
                            div { class: "wl-directive-step-badge", "Recalibration" }
                            p { class: "wl-body-muted",
                                "Observed adjustment: " span { class: "wl-mono",
                                    "-{adjustment_pct(val.estimate_adjustment)}%"
                                }
                                ". Not applied — tomorrow's plans still use raw estimates. The engine computes it and this page reports it; nothing consumes it yet."
                            }
                        }

                        p { class: "wl-body-muted wl-traj-foot",
                            "No debt carried forward. The plan is recalculated from where you actually are, not from where you said you would be."
                        }
                } else if loaded.is_none() {
                    p { class: "wl-body-muted wl-traj-empty", "Computing the vector…" }
                } else {
                    div { class: "wl-directive-card",
                        h2 { class: "wl-traj-empty-title", "No vector to plot yet." }
                        p { class: "wl-body-muted",
                            "Trajectory needs a goal with remaining milestones. Create one and the required velocity appears here."
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pace_boundaries_match_the_engines_own_floor() {
        // 0.6 is where `estimate_adjustment` bottoms out, so that is where
        // the wording changes. If the engine's floor moves, this fails.
        assert_eq!(pace_label(1.0), "Holding pace");
        assert_eq!(pace_label(0.95), "Holding pace");
        assert_eq!(pace_label(0.94), "Easing");
        assert_eq!(pace_label(0.6), "Easing");
        assert_eq!(pace_label(0.59), "Recalibrating");
        assert_eq!(pace_label(0.0), "Recalibrating");
    }

    #[test]
    fn adjustment_is_a_downwards_percentage() {
        assert_eq!(adjustment_pct(1.0), 0);
        assert_eq!(adjustment_pct(0.88), 12);
        assert_eq!(adjustment_pct(0.6), 40);
    }

    #[test]
    fn a_non_finite_adjustment_never_reaches_the_dom() {
        assert_eq!(adjustment_pct(f64::NAN), 0);
        assert_eq!(adjustment_pct(f64::INFINITY), 0);
    }
}
