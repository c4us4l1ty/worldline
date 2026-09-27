//! The control panel — what the hamburger opens.
//!
//! This was two buttons in a mostly empty sheet: "Create a goal" and a
//! gear, pinned to the bottom of a 273px panel with a void above them. The
//! void was the bug. A menu that only routes to other menus is a file
//! browser, and the product's whole premise is that you should never be
//! browsing — you should be looking at one directive.
//!
//! So the drawer stops being navigation and becomes the **map of the
//! user's operational reality**: where every active goal stands, and two
//! read-outs about the system's own behaviour. Everything here answers
//! "where am I?" without leaving the surface you are already on.
//!
//! Layout follows the reference the user pointed at (a sidebar with a
//! primary action on top, grouped rows beneath, then a "Pinned"-style
//! section at the bottom): a primary action, a System Telemetry group, an
//! Active Worldlines readout, then Settings.
//!
//! **The worldlines are not buttons.** They are a readout. Making them
//! navigable would promise a goal detail page that does not exist, and a
//! dead affordance is worse than none — the same reasoning that kept
//! directive authoring out of the compose screen (PRD delta 74).

use dioxus::prelude::*;

use crate::app::{invoke, transport, AppCtx, GoalView, Screen};

/// How many worldlines the panel shows before folding the rest into a
/// count. Four fills the panel with the telemetry group still fully
/// visible; a fifth row would push Settings off the bottom, and Settings
/// is not optional.
pub const WORLDLINE_CAP: usize = 4;

/// Fraction of a goal's milestones completed, as a 0–100 percentage.
///
/// A goal with no milestones yet is 0%, not 100% and not a divide by
/// zero. `create_goal` seeds "First steps" so this is rare, but a
/// restored store can present it, and a progress bar that fills on an
/// empty denominator is a lie.
pub fn progress_pct(done: usize, total: usize) -> f64 {
    if total == 0 {
        return 0.0;
    }
    ((done as f64 / total as f64) * 100.0).clamp(0.0, 100.0)
}

/// Split the goals into what fits and how many do not.
pub fn visible_worldlines(goals: &[GoalView]) -> (Vec<GoalView>, usize) {
    let shown = goals.len().min(WORLDLINE_CAP);
    (goals[..shown].to_vec(), goals.len() - shown)
}

#[component]
pub fn NavDrawer() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut goals = use_signal::<Option<Vec<GoalView>>>(|| None);

    // The drawer unmounts when it closes, so a plain mount effect is the
    // whole refresh story: every open re-reads. A goal created since the
    // last visit therefore appears with its bar already filled, which is
    // the whole reason this section is here.
    use_effect(move || {
        spawn(async move {
            if let Ok(v) = invoke::<Vec<GoalView>>("list_goals", ()).await {
                goals.set(Some(v));
            }
        });
    });

    let close = move |_| {
        let mut n = ctx.nav_open;
        *n.write() = false;
    };
    let go = move |screen: Screen| {
        let ctx = ctx;
        move |_| {
            {
                let mut n = ctx.nav_open;
                *n.write() = false;
            }
            {
                let mut s = ctx.screen;
                *s.write() = screen.clone();
            }
        }
    };

    let loaded = goals.read().clone();
    let (shown, hidden) = loaded
        .as_deref()
        .map(visible_worldlines)
        .unwrap_or((Vec::new(), 0));
    // Under `dx serve` every shell command is canned, so this list is
    // invented and no query ran. A fabricated map of the user's goals is
    // exactly the failure this project already paid for once with the
    // model catalog (PRD delta 148), so it says so.
    let mock = transport() == "mock";

    rsx! {
        div {
            class: "wl-modal-backdrop wl-nav-backdrop",
            onclick: close,
            role: "presentation",
            div {
                class: "wl-nav-sheet",
                role: "dialog",
                aria_label: "Control panel",
                onclick: move |e| e.stop_propagation(),

                div { class: "wl-nav-top",
                    h2 { class: "wl-nav-appname", "Worldline" }
                    button {
                        class: "wl-nav-close",
                        aria_label: "Close control panel",
                        title: "Close",
                        onclick: close,
                        crate::icons::IconBack {}
                    }
                }

                div { class: "wl-scroll-region wl-nav-scroll",
                    button {
                        class: "wl-nav-primary",
                        onclick: go(Screen::GoalCreate),
                        span { class: "wl-nav-primary-glyph", "✚" }
                        span { "Create a goal" }
                    }

                    div { class: "wl-nav-section",
                        div { class: "wl-nav-label",
                            "System Telemetry"
                            if mock {
                                span { class: "wl-nav-mock", "· MOCK" }
                            }
                        }
                        button {
                            class: "wl-nav-row",
                            onclick: go(Screen::EntropyLog),
                            span { class: "wl-nav-row-icon", crate::icons::IconEntropy {} }
                            span { class: "wl-nav-row-label", "Entropy Log" }
                            span { class: "wl-nav-chevron", "›" }
                        }
                        button {
                            class: "wl-nav-row",
                            onclick: go(Screen::Trajectory),
                            span { class: "wl-nav-row-icon", crate::icons::IconVelocity {} }
                            span { class: "wl-nav-row-label", "Trajectory" }
                            span { class: "wl-nav-chevron", "›" }
                        }
                    }

                    div { class: "wl-nav-section",
                        div { class: "wl-nav-label", "Active Worldlines" }
                        if let Some(list) = &loaded {
                            if list.is_empty() {
                                p { class: "wl-nav-empty",
                                    "No active goals. Create one and it appears here with a progress bar."
                                }
                            } else {
                                for g in shown.iter() {
                                    div { key: "{g.id}", class: "wl-worldline",
                                        div { class: "wl-worldline-top",
                                            span { class: "wl-worldline-title", "{g.title}" }
                                            span { class: "wl-worldline-count wl-mono",
                                                "{g.milestone_done} / {g.milestone_total}"
                                            }
                                        }
                                        div { class: "wl-worldline-track",
                                            div {
                                                class: "wl-worldline-fill",
                                                style: "width: {progress_pct(g.milestone_done, g.milestone_total)}%;",
                                            }
                                        }
                                    }
                                }
                                if hidden > 0 {
                                    p { class: "wl-nav-more wl-mono", "+{hidden} more" }
                                }
                            }
                        } else {
                            p { class: "wl-nav-empty", "Reading goals…" }
                        }
                    }
                }

                div { class: "wl-nav-foot",
                    button {
                        class: "wl-nav-row",
                        onclick: go(Screen::Settings),
                        span { class: "wl-nav-row-icon", crate::icons::IconSettings {} }
                        span { class: "wl-nav-row-label", "Settings" }
                        span { class: "wl-nav-chevron", "›" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal(title: &str, done: usize, total: usize) -> GoalView {
        GoalView {
            id: title.into(),
            title: title.into(),
            target_date: None,
            milestone_done: done,
            milestone_total: total,
        }
    }

    #[test]
    fn progress_never_exceeds_a_full_or_empty_bar() {
        assert_eq!(progress_pct(0, 0), 0.0);
        assert_eq!(progress_pct(0, 4), 0.0);
        assert_eq!(progress_pct(2, 4), 50.0);
        assert_eq!(progress_pct(4, 4), 100.0);
    }

    #[test]
    fn a_goal_with_no_milestones_is_empty_not_full() {
        // The divide-by-zero case a restored store can actually produce.
        assert_eq!(progress_pct(0, 0), 0.0);
    }

    #[test]
    fn more_done_than_total_cannot_overflow_the_track() {
        assert_eq!(progress_pct(9, 4), 100.0);
    }

    #[test]
    fn the_panel_caps_worldlines_and_reports_the_remainder() {
        let goals: Vec<GoalView> = (0..6).map(|i| goal(&format!("G{i}"), i, 6)).collect();
        let (shown, hidden) = visible_worldlines(&goals);
        assert_eq!(shown.len(), WORLDLINE_CAP);
        assert_eq!(hidden, 2);
        // Order is preserved: the shell returns newest-first and the
        // panel must not reshuffle it.
        assert_eq!(shown[0].title, "G0");
        assert_eq!(shown[3].title, "G3");
    }

    #[test]
    fn fewer_goals_than_the_cap_hides_nothing() {
        let goals = vec![goal("Solo", 1, 2)];
        let (shown, hidden) = visible_worldlines(&goals);
        assert_eq!(shown.len(), 1);
        assert_eq!(hidden, 0);
    }

    #[test]
    fn no_goals_yields_an_empty_panel_not_a_panic() {
        let (shown, hidden) = visible_worldlines(&[]);
        assert!(shown.is_empty());
        assert_eq!(hidden, 0);
    }
}
