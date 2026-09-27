//! Entropy Log — the task ledger.
//!
//! This page used to be the escape-hatch ledger: a read-only, grouped
//! listing of bailouts, with a pattern line (`4 events · 2 energy · 1
//! scope · 1 external`) and nothing you could do with any of it. It was
//! read-only **by decision** (PRD delta 159) because a blocked directive
//! had no unblock path in the engine, so a tap target would have been a
//! control that could not do what it said.
//!
//! Then the escape hatch was deleted (2026-09-27) — and with it the only
//! writer those rows had. A page whose entire content is a table nothing
//! appends to is worse than no page, so this became the thing the product
//! was missing: **every task, grouped by goal, with the one control that
//! resolves one.**
//!
//! # Why a task list at all
//!
//! The design system bans a scrollable list of future tasks, and the rule
//! is written against the home screen. This is the other case: a page a
//! person opens deliberately, from the control panel, to answer "what is
//! outstanding and what am I done with". The canvas still shows exactly
//! one directive and still cannot resolve it. The rule was narrowed rather
//! than broken, and PRD delta 227 records it.
//!
//! # One control per row
//!
//! A tick, and nothing else. There is no cross: "not done" would have to
//! mean something to the engine, and the honest candidate — `skipped`, a
//! velocity adjustment — turns out to need a category to be useful, which
//! is the escape hatch again under a different name. A tick is
//! unambiguous, reversible by tapping again, and the only verb the engine
//! actually implements (PRD delta 225, `Engine::mark_complete`).
//!
//! The canvas is where the task is worked and the ledger is where it is
//! resolved, which is the division the deletion of the escape hatch
//! forced and the one the page's whole shape follows from.

use dioxus::prelude::*;

use crate::app::{flash, invoke, AppCtx, CalibrationView, Screen, TaskView};
use crate::icons::IconBack;

/// What the summary line says, or nothing at all.
///
/// Two numbers, both load-bearing: how much is open, and how much of it
/// cannot start yet. A count of blocked tasks is not a failure figure — it
/// is the queue's shape, and the product's own instruction is that a
/// stalled task shows its reason rather than an alarm.
pub fn summary_line(tasks: &[TaskView]) -> String {
    let open = tasks.iter().filter(|t| !t.is_done()).count();
    if open == 0 {
        if tasks.is_empty() {
            return String::new();
        }
        return "Everything is done.".into();
    }
    let blocked = tasks.iter().filter(|t| t.is_blocked()).count();
    if blocked == 0 {
        return format!("{open} open");
    }
    format!("{open} open · {blocked} waiting on something else")
}

/// Groups the ledger under its goals, preserving first-seen order.
///
/// The shell returns open work first and the store's own order within
/// that, and goals therefore appear in the order they first have open
/// work. A bailout only ever meant something next to the thing it
/// derailed, and the same is true of a task that is waiting on another.
pub fn group_by_goal(tasks: &[TaskView]) -> Vec<(&str, Vec<&TaskView>)> {
    let mut groups: Vec<(&str, Vec<&TaskView>)> = Vec::new();
    let mut index: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for t in tasks {
        match index.get(t.goal_title.as_str()) {
            Some(&i) => groups[i].1.push(t),
            None => {
                index.insert(t.goal_title.as_str(), groups.len());
                groups.push((t.goal_title.as_str(), vec![t]));
            }
        }
    }
    groups
}

pub fn EntropyLogScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    let tasks = use_signal::<Option<Vec<TaskView>>>(|| None);
    let buckets = use_signal(Vec::new);
    let error = use_signal(|| None::<String>);
    // Ids whose tick is in flight, so a double-tap cannot fire two writes.
    // A checkbox that disables itself for the duration of its own request
    // is the whole of the "already resolved" problem.
    let pending = use_signal(Vec::new);

    // A plain mount effect is the whole refresh story: the screen unmounts
    // when it is closed, so every open re-reads. A task ticked on the
    // previous visit therefore shows as done the next time, and a goal
    // created since is here.
    // Every write below goes through `set_if_alive`. `tasks`, `error`,
    // `buckets` and `pending` are owned by THIS screen's scope, but a
    // `spawn`ed task runs at the root scope and outlives the screen, so
    // the back chevron (which is not disabled while a tick is in flight)
    // can unmount the page between the request and the reply. A direct
    // `tasks.set(..)` at that point is a `try_write().unwrap()` on a
    // dropped generational box: a panic, and wasm32 aborts on panic, so
    // the whole module dies and the window freezes on its last painted
    // frame with no error screen to explain it.
    use_effect(move || {
        spawn(async move {
            match invoke::<Vec<TaskView>>("task_ledger", ()).await {
                Ok(v) => {
                    crate::app::set_if_alive(&error, None);
                    crate::app::set_if_alive(&tasks, Some(v));
                }
                Err(e) => {
                    // A shell without the command (an older build) answers
                    // with a missing-command error, which is a real state
                    // and not a bug to hide: say so on the page.
                    crate::app::set_if_alive(&error, Some(e));
                }
            }
            // The estimator is a second query, and a failure here must not
            // take the ledger down with it — a missing Calibration card is
            // a smaller problem than a missing task list.
            if let Ok(b) = invoke::<Vec<CalibrationView>>("calibration_view", ()).await {
                crate::app::set_if_alive(&buckets, b);
            }
        });
    });

    let tick = move |(id, _title): (String, String)| {
        let mut pending = pending;
        if pending.read().iter().any(|p| p == &id) {
            return;
        }
        pending.write().push(id.clone());
        let ctx = ctx;
        spawn(async move {
            match invoke::<crate::app::DirectiveView>(
                "mark_task_done",
                serde_json::json!({ "directive_id": id }),
            )
            .await
            {
                Ok(_) => {
                    flash(&ctx, "RECORDED");
                    // Re-read rather than patch the row locally: the
                    // milestone, the canvas and the estimator all move with
                    // a tick, and a locally-patched row would be the only
                    // one of the four that did not.
                    if let Ok(v) = invoke::<Vec<TaskView>>("task_ledger", ()).await {
                        crate::app::set_if_alive(&tasks, Some(v));
                    }
                    if let Ok(b) = invoke::<Vec<CalibrationView>>("calibration_view", ()).await {
                        crate::app::set_if_alive(&buckets, b);
                    }
                    crate::screens::canvas::refresh_canvas(&ctx);
                }
                Err(e) => flash(&ctx, &format!("ERR {e}")),
            }
            // Released on both arms, and only if the page is still here.
            if crate::app::peek_if_alive(&pending).is_some() {
                let mut p = pending;
                p.write().retain(|x| x != &id);
            }
        });
    };

    let loaded = tasks.read().clone();
    let groups = loaded.as_deref().map(group_by_goal);
    let summary = loaded.as_deref().map(summary_line);
    let bucket_list = buckets.read().clone();
    let in_flight = pending.read().clone();

    rsx! {
        div { class: "wl-page",
            div { class: "wl-page-head",
                button {
                    class: "wl-circle-btn wl-back",
                    aria_label: "Back to the line",
                    title: "Back to the line",
                    onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::Canvas; } },
                    IconBack {}
                }
                h1 { class: "wl-page-title", "Entropy Log" }
            }

            div { class: "wl-scroll-region",
                if let Some(err) = error.read().clone() {
                    p { class: "wl-form-error", "{err}" }
                }
                if let Some(line) = summary {
                    if !line.is_empty() {
                        p { class: "wl-entropy-summary wl-mono", "{line}" }
                    }
                }

                if let Some(gs) = groups {
                    if gs.is_empty() {
                        div { class: "wl-directive-card",
                            h2 { class: "wl-entropy-empty-title", "Nothing is queued." }
                            p { class: "wl-body-muted",
                                "Every task from every goal is finished. This page fills in the moment a plan has work in it — it exists to mark things done, not to be visited."
                            }
                        }
                    } else {
                        for (goal, items) in gs {
                            div { key: "{goal}", class: "wl-entropy-group",
                                h2 { class: "wl-section-title wl-entropy-goal", "{goal}" }
                                for t in items.iter() {
                                    TaskRow {
                                        t: (*t).clone(),
                                        key: "{t.directive_id}",
                                        busy: in_flight.contains(&t.directive_id),
                                        on_tick: Callback::new(tick),
                                    }
                                }
                            }
                        }
                    }
                } else {
                    p { class: "wl-body-muted", "Reading the ledger…" }
                }

                // The estimator, last, and only if the user has rated
                // anything. It is a section on this page rather than a page
                // of its own because it is 200px of text about the same
                // work these rows are.
                if !bucket_list.is_empty() {
                    div { class: "wl-entropy-group wl-calibration-card",
                        h2 { class: "wl-section-title", "Calibration" }
                        p { class: "wl-body-muted",
                            "How often work you rate a given way actually lands. Your rating is the starting point; every task you mark done moves it."
                        }
                        for (b, text) in bucket_list.iter().map(|b| (b, b.line())) {
                            div { key: "{b.complexity}", class: "wl-calibration-row",
                                span { class: "wl-calibration-card-label", "{b.label}" }
                                span { class: "wl-calibration-card-value wl-mono", "{text}" }
                            }
                        }
                        p { class: "wl-body-muted wl-mono wl-traj-foot",
                            "Read into the next plan's sizing. Not applied to estimates yet."
                        }
                    }
                }
            }
        }
    }
}

/// One task, and the one control that resolves it.
#[component]
fn TaskRow(t: TaskView, busy: bool, on_tick: EventHandler<(String, String)>) -> Element {
    let meta = t.meta();
    // Joined OUTSIDE the rsx: the format string's own quotes and the
    // separator's quotes are the same character, and rsx cannot nest them.
    let meta_line = meta.join(" · ");
    let blocked = t.blocked_by.clone();
    let done = t.is_done();
    rsx! {
        div { class: "wl-task-row", class: if done { "wl-task-row--done" },
            div { class: "wl-task-body",
                p { class: "wl-task-title", "{t.title}" }
                if !meta.is_empty() {
                    p { class: "wl-task-meta wl-mono", "{meta_line}" }
                }
                // Why this task is not running, in a sentence. The whole
                // reason `blocked_by` exists: a task sitting in the queue
                // with nothing visibly wrong with it is indistinguishable
                // from a stalled app.
                if let Some(why) = blocked {
                    if !done {
                        p { class: "wl-task-blocked", "waiting on {why}" }
                    }
                }
                if let Some(body) = t.instruction.as_ref() {
                    if !body.trim().is_empty() {
                        p { class: "wl-task-instruction", "{body}" }
                    }
                }
            }
            // The tick. `aria-pressed` rather than a styled-only state, and
            // the row's own text carries "done" as well — so nothing on this
            // page depends on colour alone.
            button {
                class: "wl-circle-btn wl-task-tick",
                class: if done { "wl-task-tick--on" },
                aria_pressed: if done { "true" } else { "false" },
                aria_label: if done {
                    format!("{} is done", t.title)
                } else {
                    format!("Mark {} done", t.title)
                },
                title: if done { "Done" } else { "Mark done" },
                disabled: busy,
                onclick: move |_| on_tick.call((t.directive_id.clone(), t.title.clone())),
                crate::icons::IconCheck {}
            }
            if done {
                span { class: "wl-task-state wl-mono", "done" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The screen's own source, with the test module cut off — a scan
    /// that included the assertions could never pass, because the
    /// assertion names the string it is looking for.
    fn markup() -> &'static str {
        let source = include_str!("entropy_log.rs");
        source
            .split("#[cfg(test)]")
            .next()
            .expect("the source contains its own test module")
    }

    fn task(goal: &str, title: &str, state: &str, blocked_by: Option<&str>) -> TaskView {
        TaskView {
            directive_id: title.into(),
            goal_title: goal.into(),
            milestone_title: "M".into(),
            title: title.into(),
            estimated_minutes: 25,
            state: state.into(),
            blocked_by: blocked_by.map(str::to_string),
            ..Default::default()
        }
    }

    /// The summary has to account for every open task, or the page
    /// contradicts the rows under it.
    #[test]
    fn the_summary_counts_open_and_blocked() {
        let ledger = vec![
            task("G", "a", "queued", None),
            task("G", "b", "queued", Some("a")),
            task("G", "c", "completed", None),
        ];
        assert_eq!(
            summary_line(&ledger),
            "2 open · 1 waiting on something else"
        );
        assert_eq!(summary_line(&[]), "");
        assert_eq!(
            summary_line(&[task("G", "a", "completed", None)]),
            "Everything is done."
        );
    }

    /// A finished task is never reported as waiting on something. This is
    /// the one that bites: a stale edge on a ticked row would otherwise
    /// read as outstanding work nobody can discharge.
    #[test]
    fn a_finished_task_is_never_blocked() {
        let t = task("G", "a", "completed", Some("b"));
        assert!(!t.is_blocked());
        assert!(t.is_done());
    }

    #[test]
    fn grouping_keeps_first_seen_goal_order() {
        let ledger = vec![
            task("Second", "a", "queued", None),
            task("First", "b", "queued", None),
            task("Second", "c", "queued", None),
        ];
        let g = group_by_goal(&ledger);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].0, "Second");
        assert_eq!(g[0].1.len(), 2);
        assert_eq!(g[1].0, "First");
    }

    /// The meta line's ORDER is what a person actually reads, so it is
    /// pinned: labels first, the blocker last as a sentence of its own.
    #[test]
    fn the_meta_line_is_labels_then_the_blocker() {
        let mut t = task("G", "a", "queued", Some("b"));
        t.complexity_label = Some("heavy".into());
        t.phase = Some((2, 4));
        assert_eq!(
            t.meta(),
            vec!["M", "25 min", "step 2 of 4", "heavy"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        // A monolithic, unrated task shows no phase and no word rather
        // than empty placeholders.
        let plain = task("G", "a", "queued", None);
        assert_eq!(plain.meta(), vec!["M".to_string(), "25 min".to_string()]);
    }

    /// The calibration line must say "no record yet" rather than quoting a
    /// percentage the user would read as a measurement.
    #[test]
    fn an_unmeasured_bucket_says_so_in_words() {
        let b = CalibrationView {
            complexity: 4,
            label: "heavy".into(),
            percent: 63,
            evidence: "0 of 0".into(),
            observations: 0,
        };
        assert_eq!(b.line(), "heavy · no record yet");
        let measured = CalibrationView {
            observations: 9,
            evidence: "4 of 9".into(),
            ..b
        };
        assert_eq!(measured.line(), "heavy · 4 of 9 finished · 63%");
    }

    /// The page has exactly one control and it is the tick. A cross here
    /// would be a control with no engine transition behind it, which is the
    /// failure PRD delta 159 was written about.
    #[test]
    fn the_page_offers_a_tick_and_nothing_else() {
        let source = markup();
        assert!(
            !source.contains("bail_out"),
            "the escape hatch is gone; the ledger must not call it"
        );
        assert!(
            !source.contains("complete_directive"),
            "and it must not reach the canvas's removed completion path either"
        );
        // The one command it does issue that resolves anything.
        assert!(source.contains("mark_task_done"));
    }
}
