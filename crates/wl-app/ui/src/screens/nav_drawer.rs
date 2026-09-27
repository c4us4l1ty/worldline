//! The control panel — what the hamburger opens.
//!
//! This was two buttons in a mostly empty sheet: "Create a goal" and a
//! gear, pinned to the bottom of a 273px panel with a void above them. The
//! void was the bug. A menu that only routes to other menus is a file
//! browser, and the product's whole premise is that you should never be
//! browsing — you should be looking at one directive.
//!
//! So the panel stops being navigation and becomes the **map of the
//! user's operational reality**: where every active goal stands, and two
//! read-outs about the system's own behaviour. Everything here answers
//! "where am I?" without leaving the surface you are already on.
//!
//! The sheet is FULL HEIGHT — top edge to bottom edge of the 747px frame.
//! An intermediate build made it hug its content and stop halfway down,
//! and the user rejected it: a drawer that ends mid-air leaves the canvas
//! visible beneath it, and there is no way to tell from the outside whether
//! the tap missed or the surface is broken. Full height, with the worldline
//! readout absorbing the slack so the extra space lands inside the list
//! region rather than becoming a void below the panel.
//!
//! The 2026-09-27 redesign fixed three things that made it read as
//! inconsistent rather than as a system:
//!
//! * **The group labels were louder than the rows they headed.** The
//!   `wl-nav-label` rule set `color: var(--wl-text-tertiary)`, and that
//!   token is used thirteen times in `wl.css` and **defined in none of
//!   them**. A declaration naming an undefined custom property is
//!   invalid at computed-value time, so the property inherited — and
//!   "SYSTEM TELEMETRY" rendered at full `--wl-text-primary` linen,
//!   brighter than the 15px rows below it. The token does not exist any
//!   more; group labels are the mono eyebrow at the 12px floor in
//!   `--wl-text-muted`, which is genuinely quieter than what it labels.
//! * **The sheet was 273px for 15px rows**, leaving ~200px of label
//!   column after the icon gutter — too narrow for a goal title and a
//!   count on one line. It is 312px now.
//! * **The primary action and the trailing chevrons were font glyphs**
//!   (`✚`, `›`), so their weight and colour came from the font stack
//!   rather than from the theme. Both are inline SVG like everything
//!   else in the app.
//!
//! **The worldlines are not buttons.** They are a readout. Making them
//! navigable would promise a goal detail page that does not exist, and a
//! dead affordance is worse than none — the same reasoning that kept
//! directive authoring out of the compose screen (PRD delta 74).

use dioxus::prelude::*;

use crate::app::{invoke, transport, AppCtx, GoalView, Screen};
// Imported for their side effect on the macro namespace rather than by
// name: these are referenced as `crate::icons::Icon… {}` at the call site
// so the markup reads as the thing it renders, and a `use` here would be an
// unused import warning for a type that is in fact used.
#[allow(unused_imports)]
use crate::icons::{IconBack, IconChevron, IconEntropy, IconNew, IconSettings, IconVelocity};

/// How many worldlines the panel shows before folding the rest into a
/// count. Three fills the panel with the telemetry group still fully
/// visible; a fourth row would push Settings off the bottom, and Settings
/// is not optional.
pub const WORLDLINE_CAP: usize = 3;

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
    let goals = use_signal::<Option<Vec<GoalView>>>(|| None);

    // The drawer unmounts when it closes, so a plain mount effect is the
    // whole refresh story: every open re-reads. A goal created since the
    // last visit therefore appears with its bar already filled, which is
    // the whole reason this section is here.
    use_effect(move || {
        spawn(async move {
            // `goals` belongs to the DRAWER's scope, and this task does
            // not: it runs at the root scope and outlives the panel. The
            // window between tapping a row and the reply is exactly when
            // the panel unmounts, so the write has to be fallible — see
            // `app::set_if_alive`. A `goals.set(..)` here aborted the
            // wasm module, freezing the window on its last frame.
            if let Ok(v) = invoke::<Vec<GoalView>>("list_goals", ()).await {
                crate::app::set_if_alive(&goals, Some(v));
            }
        });
    });

    // Every close in this file goes through `AppCtx::close_nav`, and every
    // open goes through `AppCtx::open_nav`. They used to be bare
    // `*s.write() = …` on the signal, which is a write with no name on it:
    // at the backdrop, at the close button, and at each of the four rows,
    // "close the panel" looked identical to "flip the panel" and only
    // reading the boolean's polarity told them apart. A named verb is the
    // difference between a menu that opens and a menu that depends on
    // which boolean a given handler happened to write.
    let close = move |_| ctx.close_nav();
    let go = move |screen: Screen| {
        let ctx = ctx;
        move |_| {
            // Close BEFORE routing. Both writes land in the same render,
            // so the order here is not about frames — it is that a panel
            // left open over a screen it does not belong to is a state no
            // key can reach, because the screen's own handlers do not know
            // the panel exists.
            ctx.close_nav();
            let mut s = ctx.screen;
            *s.write() = screen.clone();
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
                    h2 { class: "wl-nav-wordmark", "Worldline" }
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
                        span { class: "wl-nav-primary-glyph", crate::icons::IconNew {} }
                        span { "New goal" }
                    }

                    div { class: "wl-nav-group",
                        div { class: "wl-nav-label",
                            "System"
                            if mock {
                                span { class: "wl-nav-mock", "· MOCK" }
                            }
                        }
                        button {
                            class: "wl-nav-row",
                            onclick: go(Screen::EntropyLog),
                            span { class: "wl-nav-row-icon", crate::icons::IconEntropy {} }
                            span { class: "wl-nav-row-label", "Entropy Log" }
                            span { class: "wl-nav-chevron", crate::icons::IconChevron {} }
                        }
                        button {
                            class: "wl-nav-row",
                            onclick: go(Screen::Trajectory),
                            span { class: "wl-nav-row-icon", crate::icons::IconVelocity {} }
                            span { class: "wl-nav-row-label", "Trajectory" }
                            span { class: "wl-nav-chevron", crate::icons::IconChevron {} }
                        }
                    }

                    // `.wl-nav-worldlines` is what makes the full-height
                    // sheet not a full-height VOID: this group absorbs the
                    // leftover height, so the empty space sits inside the
                    // list region — where a list is expected to have room —
                    // instead of below the whole panel. See wl.css.
                    div { class: "wl-nav-group wl-nav-worldlines",
                        div { class: "wl-nav-label", "Active worldlines" }
                        if let Some(list) = &loaded {
                            if list.is_empty() {
                                p { class: "wl-nav-empty",
                                    "No active goals yet. Create one and it appears here."
                                }
                            } else {
                                for g in shown.iter() {
                                    div { key: "{g.id}", class: "wl-worldline",
                                        div { class: "wl-worldline-top",
                                            span { class: "wl-worldline-title", "{g.title}" }
                                            span { class: "wl-worldline-count",
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
                                    p { class: "wl-nav-more", "+{hidden} more" }
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
                        span { class: "wl-nav-chevron", crate::icons::IconChevron {} }
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
        assert_eq!(hidden, 3);
        // Order is preserved: the shell returns newest-first and the
        // panel must not reshuffle it.
        assert_eq!(shown[0].title, "G0");
        assert_eq!(shown[2].title, "G2");
    }

    /// The cap is load-bearing, not a preference. It is the only thing
    /// keeping Settings — which is pinned to the floor of a 747px sheet
    /// and is not optional — on screen when a user has six goals. Three
    /// rows plus their bars is the most that fits under both group
    /// headers with the panel scrolled to the top; a fourth pushes the
    /// readout into the scroll region for no benefit, since there is no
    /// goal detail page to navigate to anyway.
    #[test]
    fn the_cap_leaves_room_for_the_groups_above_and_settings_below() {
        // The cap is a constant, so its bounds are checked at compile
        // time rather than by a test that can only fail after a build.
        const {
            assert!(WORLDLINE_CAP >= 2, "one row is not a readout");
            assert!(
                WORLDLINE_CAP <= 4,
                "past four, the panel scrolls and Settings falls off the bottom"
            );
        }
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
