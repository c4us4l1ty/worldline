//! The plan preview — the dependency graph, and the only thing between
//! the architect and the daily queue.
//!
//! **Nothing is written until Commit.** The plan lives in
//! `AppCtx::plan` — a signal, not a table — so backing out of this screen
//! leaves no goal, no milestone and no task behind, and closing the app
//! mid-review loses the draft rather than leaving a half-plan in the
//! database. That is the whole reason the Tier-1 call was split in two
//! (see `goal_create.rs`); when it was one call, "let me look at it first"
//! was not expressible.
//!
//! # The graph is a spine, not a force layout
//!
//! At 420×747 a force-directed graph is unreadable and a layered one gets
//! worse fast: five milestones of four tasks each is twenty nodes and
//! fifteen edges with no room to draw either. So the ORDER is the critical
//! path — a vertical spine, one node per task in the order it runs — and
//! an edge is drawn as a small marker on the node it points *into*, saying
//! "this waits for that one". Reading top to bottom is reading the
//! sequence, which is the thing a person is actually being asked to
//! approve.
//!
//! # Every field is editable in place
//!
//! The reference this follows is a generated answer you can correct rather
//! than re-roll: a card expands, the text becomes a field, and Save puts
//! it back. A full re-generate costs a billable call and throws away the
//! other four things you were happy with; a text edit costs nothing.

use dioxus::prelude::*;

use crate::app::{flash, invoke, AppCtx, PlanDraft, PlanStage, PlanStep, Screen};
use crate::icons::IconBack;

/// How a task sits in the graph, derived from its `after`.
///
/// Two states and no more. A task either waits for something or it does
/// not, and a third state ("waits for something that itself waits") is
/// arithmetic a person should not have to do while looking at a picture
/// of their own plan.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeState {
    /// Nothing blocks it — this one runs next.
    Ready,
    /// It has a prerequisite, named in the marker.
    Waiting,
}

pub fn node_state(step: &PlanStep) -> NodeState {
    if step.after.is_some() {
        NodeState::Waiting
    } else {
        NodeState::Ready
    }
}

/// The label on a waiting node: "after <title>".
///
/// Empty when the node is ready, so the row reserves nothing for a
/// sentence it does not have.
pub fn waits_on(step: &PlanStep, plan: &PlanDraft) -> String {
    let Some(after) = step.after else {
        return String::new();
    };
    match plan.steps().nth(after) {
        // A step whose index points past the end is a malformed payload, and
        // naming the index is more use than naming nothing.
        Some(other) => format!("after {}", other.title),
        None => format!("after task {after}"),
    }
}

/// The spine's row count, for the scroll region's own bookkeeping.
pub fn graph_rows(plan: &PlanDraft) -> usize {
    plan.milestones.iter().map(|m| 1 + m.steps.len()).sum()
}

pub fn PlanPreviewScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    let error = use_signal(|| None::<String>);
    // Which card is open, and which card is being edited. Separate signals
    // because "showing the steps" and "let me change the text" are
    // different intents, and one `Option<usize>` for both would close the
    // card every time a field took focus.
    let open = use_signal(|| None::<usize>);
    let mut editing = use_signal(|| None::<usize>);
    // Draft text for the step being edited, held separately from the plan
    // so a half-typed title is not the plan's title until Save.
    let mut draft_title = use_signal(String::new);
    let mut draft_minutes = use_signal(String::new);

    let plan = ctx.plan.read().clone();

    let begin_edit = move |(index, step): (usize, PlanStep)| {
        // Seed the draft only when the editor is moving to a DIFFERENT
        // step. It re-seeded on every call, and the title field's
        // `oninput` calls it on every keystroke — so typing a new title
        // reset the estimate field to the step's stored value
        // mid-keystroke. That is invisible until the estimate field is
        // given the `oninput` it was missing, at which point it becomes
        // "you cannot change both at once", which is not a feature.
        if *editing.peek() != Some(index) {
            draft_title.set(step.title.clone());
            draft_minutes.set(step.estimated_minutes.to_string());
        }
        editing.set(Some(index));
    };

    let save_edit = move |()| {
        let Some(index) = *editing.peek() else { return };
        let title = draft_title.read().trim().to_string();
        let minutes: i64 = match draft_minutes.read().trim().parse() {
            Ok(v) => v,
            Err(_) => {
                flash(&ctx, "THE ESTIMATE MUST BE A NUMBER OF MINUTES");
                return;
            }
        };
        // The bound is the store's, checked here so a bad number is
        // refused where it was typed rather than arriving as a failed
        // commit after the user approved a plan.
        if let Err(why) = crate::domain::check_minutes("estimate", minutes) {
            flash(&ctx, &why);
            return;
        }
        if title.is_empty() {
            flash(&ctx, "A TASK NEEDS A NAME");
            return;
        }
        if title.chars().count() > crate::domain::MAX_TITLE_CHARS {
            flash(&ctx, "THAT NAME IS TOO LONG");
            return;
        }
        let mut p = ctx.plan;
        {
            let mut guard = p.write();
            let Some(plan) = guard.as_mut() else { return };
            plan.rename_step(index, &title);
            if plan.set_step_minutes(index, minutes).is_err() {
                drop(guard);
                flash(&ctx, "THAT ESTIMATE CANNOT BE USED");
                return;
            }
        }
        editing.set(None);
    };

    let commit = move |_| {
        if *ctx.plan_stage.read() == PlanStage::Writing {
            return;
        }
        let Some(plan) = ctx.plan.read().clone() else {
            return;
        };
        if plan.step_count() == 0 {
            flash(&ctx, "THERE IS NOTHING TO COMMIT");
            return;
        }
        {
            let mut s = ctx.plan_stage;
            s.set(PlanStage::Writing);
        }
        let ctx = ctx;
        let date = crate::app::date_plus_days(&crate::app::today_local(), 0);
        spawn(async move {
            #[derive(serde::Serialize)]
            struct Commit {
                // The shell's own `PlanPreview`, translated by
                // `PlanDraft::to_preview`. It used to be the `PlanDraft`
                // itself, on the reasoning that a field this build does
                // not know about would cross intact — but the shell
                // binds a DIFFERENT type, and the two disagree on
                // `steps`/`directives`, `instruction`/`execution_context`
                // and a flat `repair` list versus `{ notes }`. Since
                // `PlanPreview.plan` has no `#[serde(default)]`, the
                // mismatch failed argument binding outright and no plan
                // could ever be committed.
                preview: crate::app::PlanPreviewWire,
                target_date: Option<String>,
                intent: Option<String>,
            }
            // `preview` is a STRUCT parameter, so the shell needs it
            // wrapped — see the `rename_all` note at the top of
            // commands.rs. Sending it flat fails deserialisation loudly;
            // sending the wrong inner shape did not, which is what
            // happened.
            let req = Commit {
                preview: plan.to_preview(),
                target_date: date,
                intent: None,
            };
            #[derive(serde::Deserialize, Default)]
            struct Outcome {
                warnings: Vec<String>,
            }
            match invoke::<Outcome>("commit_plan", req).await {
                Ok(out) => {
                    let mut s = ctx.plan_stage;
                    s.set(PlanStage::Idle);
                    // The plan is spent. Leaving it in the signal would
                    // mean a second Commit writes a second goal.
                    let mut spent = ctx.plan;
                    spent.write().take();
                    if out.warnings.is_empty() {
                        flash(&ctx, "ON THE LINE");
                    } else {
                        // A receipt, not a log: the user approved a
                        // preview, and these are the places the written
                        // plan differs from it. Naming them is the only
                        // honest option.
                        flash(
                            &ctx,
                            &format!("ON THE LINE — {} ADJUSTED", out.warnings.len()),
                        );
                    }
                    crate::screens::canvas::refresh_canvas(&ctx);
                    let mut sc = ctx.screen;
                    *sc.write() = Screen::Canvas;
                }
                Err(e) => {
                    let mut s = ctx.plan_stage;
                    s.set(PlanStage::Idle);
                    // Fallible: Commit is a local write, but the user can
                    // still have navigated away while it ran, and a
                    // dropped scope is a panic (see `app::set_if_alive`).
                    crate::app::set_if_alive(&error, Some(e));
                }
            }
        });
    };

    let discard = move |_| {
        // `Signal::set` needs `&mut`, and the signal is reached through a
        // `Copy` context — so the handle is bound locally rather than the
        // whole `AppCtx` being made mutable for one line.
        let mut plan_signal = ctx.plan;
        plan_signal.write().take();
        let mut s = ctx.screen;
        *s.write() = Screen::GoalCreate;
    };

    let stage = *ctx.plan_stage.read();
    let open_v = *open.read();
    let editing_v = *editing.read();
    let plan_line = plan.as_ref().map(|p| {
        format!(
            "{} TASKS · RATED {}/5 {}",
            p.step_count(),
            p.complexity,
            crate::screens::goal_create::complexity_word(p.complexity)
        )
    });

    // Hoisted out of the node loop below: both are loop-invariant, and
    // reading them inside allocated two `String`s per node on every
    // render — forty needless allocations per character typed in the
    // edit field, for a twenty-step plan.
    let draft_title_v = draft_title.read().clone();
    let draft_minutes_v = draft_minutes.read().clone();

    rsx! {
        div { class: "wl-page",
            div { class: "wl-page-head",
                button {
                    class: "wl-circle-btn wl-back",
                    aria_label: "Back to the objective",
                    title: "Back",
                    disabled: stage.is_busy(),
                    onclick: discard,
                    IconBack {}
                }
                h1 { class: "wl-page-title", "The plan" }
            }

            if let Some(plan) = plan.clone() {
                div { class: "wl-scroll-region",
                    // What the shell repaired, and the seeded stand-in's
                    // reason. Above everything, because it is the reason
                    // the rest of this page is worth reading carefully.
                    if let Some(why) = plan.fallback.clone() {
                        p { class: "wl-form-error", "{why}" }
                    }
                    if !plan.repair.is_empty() {
                        div { class: "wl-plan-repair",
                            p { class: "wl-body-muted wl-mono",
                                "{plan.repair.len()} ADJUSTED BEFORE COMMIT"
                            }
                            for note in plan.repair.iter() {
                                p { class: "wl-body-muted", "{note}" }
                            }
                        }
                    }

                    p { class: "wl-plan-title", "{plan.title}" }
                    if let Some(line) = plan_line.clone() {
                        p { class: "wl-body-muted wl-mono", "{line}" }
                    }

                    // The graph: the critical path as a vertical spine,
                    // with each task's prerequisite as a marker on the node
                    // it points into.
                    //
                    // The flat index is computed ONCE, before the markup,
                    // because an `after` is a position in the whole plan and
                    // counting it inside the loop is how a node ends up
                    // editing the wrong task.
                    div { class: "wl-graph", role: "list",
                        for row in flatten(&plan) {
                            if row.heading {
                                div { key: "m-{row.title}", class: "wl-graph-milestone", role: "listitem",
                                    div { class: "wl-section-title", "{row.title}" }
                                    if let Some(why) = row.rationale.clone() {
                                        // One sentence, from the model, about
                                        // WHY this sits here. It is the thing a
                                        // person disagrees with, and it is far
                                        // easier to disagree with a sentence
                                        // than with a shape.
                                        p { class: "wl-graph-why", "{why}" }
                                    }
                                }
                            } else {
                                TaskNode {
                                    key: "t-{row.index}",
                                    step: row.step.clone(),
                                    index: row.index,
                                    waits_on: waits_on(row.step, &plan),
                                    is_open: open_v == Some(row.index),
                                    is_editing: editing_v == Some(row.index),
                                    draft_title: draft_title_v.clone(),
                                    draft_minutes: draft_minutes_v.clone(),
                                    disabled: stage.is_busy(),
                                    on_toggle: move |i: usize| {
                                        let mut o = open;
                                        o.set(if *o.peek() == Some(i) { None } else { Some(i) });
                                    },
                                    on_edit: begin_edit,
                                    on_edit_minutes: {
                                        let mut d = draft_minutes;
                                        move |v: String| d.set(v)
                                    },
                                    on_save: save_edit,
                                    on_cancel_edit: move |()| editing.set(None),
                                }
                            }
                        }
                    }

                    if let Some(msg) = error.read().clone() {
                        p { class: "wl-form-error", "{msg}" }
                    }

                    div { class: "wl-actions-row",
                        button { class: "wl-btn-primary", disabled: stage.is_busy(),
                            onclick: commit,
                            if stage == PlanStage::Writing { "Writing…" } else { "Commit to the line" }
                        }
                        button { class: "wl-btn-ghost", disabled: stage.is_busy(),
                            onclick: discard,
                            "Discard"
                        }
                    }
                    // The cost of staging, stated once and plainly: nothing
                    // above this line exists in the database, so backing out
                    // is free and there is nothing to clean up.
                    p { class: "wl-hint wl-traj-foot",
                        "Nothing is saved until you commit. Change anything here first."
                    }
                }
            } else {
                div { class: "wl-scroll-region",
                    // The one way to reach this screen without a plan is
                    // the nav drawer, and it must say so rather than
                    // render an empty frame.
                    p { class: "wl-body-muted",
                        "There is no plan to show. Generate one from the menu."
                    }
                }
            }
        }
    }
}

/// One row of the spine: a milestone heading, or a task.
///
/// A struct rather than a tuple because four of the five fields are
/// `String`/`Option` and a reader cannot tell them apart — which is how
/// `row.1` ends up meaning "the index" in one place and "the title" in
/// another.
///
/// Flattened with the plan-wide index up front, because an `after` is a
/// position in the WHOLE plan and the shell's `commit_plan` resolves it
/// that way. Recomputing the index inside the render loop is how a tap
/// ends up editing a different task than the one under the finger.
pub struct SpineRow<'a> {
    /// A milestone heading rather than a task.
    pub heading: bool,
    /// Plan-wide task index. Meaningless on a heading.
    pub index: usize,
    pub title: String,
    pub rationale: Option<String>,
    pub step: &'a PlanStep,
}

/// Borrowed by heading rows, which carry no task of their own. A
/// milestone with no steps is a payload the repair pass should have caught;
/// the heading still has to render.
static EMPTY_STEP: PlanStep = PlanStep {
    title: String::new(),
    instruction: None,
    estimated_minutes: 0,
    after: None,
    phases: Vec::new(),
    edited: false,
};

/// Milestone headings and their tasks, in running order.
pub fn flatten(plan: &PlanDraft) -> Vec<SpineRow<'_>> {
    let mut out = Vec::new();
    let mut index = 0usize;
    for m in &plan.milestones {
        // A milestone with no tasks still gets its heading: the repair pass
        // drops those before they reach here, so an empty one is a
        // malformed payload, and showing the heading is how a person sees
        // that something is missing rather than finding a gap in the graph.
        out.push(SpineRow {
            heading: true,
            index: 0,
            title: m.title.clone(),
            rationale: m.rationale.clone(),
            step: &EMPTY_STEP,
        });
        for step in &m.steps {
            out.push(SpineRow {
                heading: false,
                index,
                title: String::new(),
                rationale: None,
                step,
            });
            index += 1;
        }
    }
    out
}

/// One node on the spine, and its card.
#[component]
fn TaskNode(
    step: PlanStep,
    index: usize,
    waits_on: String,
    is_open: bool,
    is_editing: bool,
    draft_title: String,
    draft_minutes: String,
    disabled: bool,
    on_toggle: EventHandler<usize>,
    on_edit: EventHandler<(usize, PlanStep)>,
    /// The estimate field's own keystrokes. A separate handler from
    /// `on_edit` because that one carries a whole `PlanStep` and is
    /// fired by the TITLE field on every keystroke; reusing it here
    /// would re-seed the estimate draft from the stored step mid-typing
    /// (see `begin_edit`).
    on_edit_minutes: EventHandler<String>,
    on_save: EventHandler<()>,
    on_cancel_edit: EventHandler<()>,
) -> Element {
    let state = node_state(&step);
    rsx! {
        div {
            class: "wl-graph-node",
            class: if state == NodeState::Waiting { "wl-graph-node--waiting" },
            role: "listitem",
            // The spine dot is the "you are here" mark. A 4px cream dot on
            // a 2px line, and the line is `--wl-border-strong` so the graph
            // reads as structure rather than as a list of bullets.
            span { class: "wl-graph-dot", aria_hidden: "true" }
            div { class: "wl-directive-card wl-task-card",
                div { class: "wl-task-card-top",
                    // Collapsed: the title, the metric on the right, and a
                    // chevron. The pattern the reference uses, and it is
                    // the right one — a plan is long and the scan is for
                    // the title.
                    if !is_editing {
                        button {
                            class: "wl-task-card-summary",
                            aria_expanded: if is_open { "true" } else { "false" },
                            disabled: disabled,
                            onclick: move |_| on_toggle.call(index),
                            span { class: "wl-task-title", "{step.title}" }
                            span { class: "wl-task-metric wl-mono",
                                "{step.estimated_minutes} min"
                            }
                            span { class: "wl-value-row-go", crate::icons::IconChevron {} }
                        }
                    } else {
                        span { class: "wl-task-title", "{step.title}" }
                    }
                }
                // The edge, said in words on the node it points into. A
                // dashed rail rather than a drawn elbow: at this width an
                // elbow is a box, and a box is a control.
                if !waits_on.is_empty() && !is_editing {
                    p { class: "wl-graph-edge", "— {waits_on}" }
                }
                if is_editing {
                    div { class: "wl-task-edit",
                        input {
                            class: "wl-input",
                            r#type: "text",
                            maxlength: crate::domain::MAX_TITLE_CHARS,
                            "aria-label": "Task name",
                            value: "{draft_title}",
                            // The field hands back a whole `PlanStep`, and
                            // the other fields are carried from the one on
                            // screen so the edit does not blank the estimate
                            // while the title is being typed.
                            oninput: {
                                let step = step.clone();
                                move |e: Event<FormData>| on_edit.call((index, PlanStep {
                                    title: e.value(),
                                    ..step.clone()
                                }))
                            },
                        },
                        div { class: "wl-task-edit-row",
                            input {
                                class: "wl-input wl-input--mono",
                                r#type: "text",
                                inputmode: "numeric",
                                "aria-label": "Estimate in minutes",
                                value: "{draft_minutes}",
                                // Wired, which it was not: the field
                                // rendered `draft_minutes` and no handler
                                // ever wrote back, so `save_edit` parsed
                                // the ORIGINAL number and the typed
                                // estimate was discarded with no error.
                                // The title field two nodes up has had an
                                // `oninput` all along, which is what made
                                // the omission visible.
                                oninput: move |e: Event<FormData>| on_edit_minutes.call(e.value()),
                            },
                        },
                    }
                }
                if is_open && !is_editing {
                    if let Some(body) = step.instruction.clone() {
                        p { class: "wl-body-muted wl-task-instruction", "{body}" }
                    }
                    if !step.phases.is_empty() {
                        div { class: "wl-task-phases",
                            p { class: "wl-eyebrow", "Steps" }
                            for p in step.phases.iter() {
                                div { class: "wl-task-phase wl-mono", "{p.minutes} min · {p.title}" }
                            }
                        }
                    }
                    // The inline edit, phrased as the reference phrases it:
                    // a correction is cheaper than a re-roll.
                    button {
                        class: "wl-btn-ghost wl-btn-compact",
                        disabled: disabled,
                        onclick: move |_| on_edit.call((index, step.clone())),
                        "Something off? Change it"
                    }
                }
                if is_editing {
                    div { class: "wl-inline-actions",
                        button { class: "wl-btn-primary wl-btn-compact",
                            disabled: disabled, onclick: move |_| on_save.call(()), "Save" }
                        button { class: "wl-btn-ghost wl-btn-compact",
                            disabled: disabled, onclick: move |_| on_cancel_edit.call(()), "Discard" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::PlanMilestone;

    /// The screen's own source, with the test module cut off — a scan
    /// that included the assertions could never pass, because the
    /// assertion names the string it is looking for.
    fn markup() -> &'static str {
        let source = include_str!("plan_preview.rs");
        source
            .split("#[cfg(test)]")
            .next()
            .expect("the source contains its own test module")
    }

    fn plan(steps: Vec<PlanStep>) -> PlanDraft {
        PlanDraft {
            title: "G".into(),
            milestones: vec![PlanMilestone {
                title: "M".into(),
                rationale: None,
                steps,
            }],
            complexity: 3,
            ..Default::default()
        }
    }

    fn step(title: &str, after: Option<usize>) -> PlanStep {
        PlanStep {
            title: title.into(),
            estimated_minutes: 20,
            after,
            ..Default::default()
        }
    }

    /// A node is either waiting or it is not. The label names the task it
    /// waits on, because "waiting" alone does not tell you what to go and
    /// do.
    #[test]
    fn a_waiting_node_names_what_it_waits_on() {
        let p = plan(vec![step("a", None), step("b", Some(0))]);
        let steps: Vec<&PlanStep> = p.steps().collect();
        assert_eq!(node_state(steps[0]), NodeState::Ready);
        assert_eq!(node_state(steps[1]), NodeState::Waiting);
        assert_eq!(waits_on(steps[0], &p), "");
        assert_eq!(waits_on(steps[1], &p), "after a");
    }

    /// An index past the end is a malformed payload, and the label says so
    /// rather than rendering an empty marker.
    #[test]
    fn a_dangling_edge_is_named_rather_than_hidden() {
        let p = plan(vec![step("a", None), step("b", Some(9))]);
        let steps: Vec<&PlanStep> = p.steps().collect();
        assert_eq!(waits_on(steps[1], &p), "after task 9");
        assert_eq!(node_state(steps[1]), NodeState::Waiting);
    }

    /// The spine's length is what the scroll region has to hold. One
    /// heading per milestone plus one node per task.
    #[test]
    fn the_spine_is_a_heading_and_a_node_per_thing() {
        let p = plan(vec![step("a", None), step("b", Some(0))]);
        assert_eq!(graph_rows(&p), 3, "one milestone heading, two nodes");
        assert_eq!(graph_rows(&PlanDraft::default()), 0);
    }

    /// An edit has to be visible in the plan, or the preview is showing
    /// something other than what would be committed.
    #[test]
    fn an_edit_is_visible_in_the_draft() {
        let mut p = plan(vec![step("a", None)]);
        p.rename_step(0, "Draft the outline");
        assert!(p.steps().next().unwrap().edited);
        assert_eq!(p.steps().next().unwrap().title, "Draft the outline");
        // A rename of an index past the end is a no-op, not a panic.
        p.rename_step(9, "x");
        assert_eq!(p.steps().count(), 1);
    }

    #[test]
    fn the_preview_holds_no_own_copy_of_the_plan() {
        // The plan is in `AppCtx::plan`, so leaving for Settings and coming
        // back keeps the edits. A screen that re-fetched would discard
        // them, and the user would find out by losing work.
        let source = markup();
        assert!(
            !source.contains("master_plan_preview"),
            "the preview must never re-call the billable endpoint; the plan arrives in the signal"
        );
        assert!(
            source.contains("commit_plan"),
            "and Commit is the only command it issues"
        );
    }

    /// The spine carries the plan-wide index up front, so a tap edits the
    /// task under the finger. Two milestones with the same first task
    /// title is the case that distinguishes it from counting per milestone.
    #[test]
    fn the_spine_indexes_continuously_across_milestones() {
        let p = PlanDraft {
            milestones: vec![
                PlanMilestone {
                    title: "One".into(),
                    rationale: None,
                    steps: vec![step("a", None), step("b", Some(0))],
                },
                PlanMilestone {
                    title: "Two".into(),
                    rationale: None,
                    steps: vec![step("c", Some(1))],
                },
            ],
            ..Default::default()
        };
        let rows = flatten(&p);
        let tasks: Vec<(usize, String)> = rows
            .iter()
            .filter(|r| !r.heading)
            .map(|r| (r.index, r.step.title.clone()))
            .collect();
        assert_eq!(
            tasks,
            vec![(0, "a".into()), (1, "b".into()), (2, "c".into())],
            "the second milestone's first task is index 2, not 0"
        );
        // …and the edge across the boundary still names the right task.
        assert_eq!(waits_on(p.steps().nth(2).unwrap(), &p), "after b");
    }
}
