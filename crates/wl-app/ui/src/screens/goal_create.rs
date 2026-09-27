//! Goal creation — the compose screen.
//!
//! One free-text field owns the page. The user types intent in their own
//! words ("in n out burger"); the architect names the goal and builds the
//! milestone hierarchy. The manual path uses the same text directly and
//! needs no API key.
//!
//! The screen deliberately borrows the canvas's premise — ONE thing, no
//! chrome — rather than the settings page's four-cards-and-a-save shape.
//! There is no title field, no details field, and no constraints field:
//! `wl-core`'s `fallback_title` derives a name from the first line when the
//! architect does not offer one, and the Tier-1 prompt is told to read the
//! whole text as intent.
//!
//! # Generate does not write anything (2026-09-27)
//!
//! It used to. One click made a billable request, parsed the plan and
//! persisted the whole tree in the same call, so "let me look at it first"
//! was not expressible — and when the plan failed validation the user was
//! left with nothing at all, which is the "the AI doesn't make any task for
//! me" report. Generate now fetches a [`PlanDraft`] and hands it to the
//! preview; only Commit writes.
//!
//! # The three things this page can be wrong about, and how each is caught
//!
//! * **Not configured.** A missing model or key used to be discovered from
//!   a 2.2-second toast, or from a provider error after the request had
//!   already gone out. `ai_readiness` is a local, free, instant check and
//!   its `missing` string is the button's disabled reason.
//! * **The call never answers.** The button is Cancel-able, the stage line
//!   is the command actually being awaited, and the shell's own 75-second
//!   deadline turns a wedged request into a sentence rather than a spinner.
//! * **The plan is wrong.** The preview shows it as a dependency graph and
//!   every field is editable in place, before anything is written.

use dioxus::prelude::*;

use crate::app::{
    flash, invoke, AiReadiness, AppCtx, PlanDraft, PlanMilestone, PlanPhase, PlanStage, PlanStep,
    Screen,
};
use crate::icons::IconBack;

/// Hard cap on the compose field. Matches `domain::MAX_DESCRIPTION_CHARS`
/// so the shell accepts anything the UI lets you type — a client-side cap
/// that disagreed with the store's would either block valid prose or let a
/// request through that fails at the write boundary after the BYOK call.
const MAX_INTENT_CHARS: usize = 4000;

/// The five *Estimated Complexity* stops, in rating order.
///
/// The words, not numbers, are what the user chooses by — the number is
/// what the estimator buckets on, and the pair is a mapping rather than a
/// coincidence. `light+` exists because "between light and standard" has
/// no single-word synonym and every other label is one word.
pub const COMPLEXITY_STOPS: [&str; 5] = ["light", "light+", "standard", "heavy", "deep"];

/// Word for a rating, or the middle stop for anything out of range.
///
/// Clamped rather than rejected: the control cannot produce an
/// out-of-range value, and a `None` here would mean a slider with no
/// selection on a fresh screen.
pub fn complexity_word(rating: i64) -> &'static str {
    const {
        assert!(COMPLEXITY_STOPS.len() == 5, "the scale is five stops");
    }
    let i = (rating.clamp(1, 5) - 1) as usize;
    COMPLEXITY_STOPS[i]
}

/// How far along the track a rating sits, 0–100.
///
/// Percentages rather than fractions so the fill can be a `width%` with no
/// unit conversion in CSS, and so the value is a plain number in a test.
pub fn complexity_fill(rating: i64) -> f64 {
    let clamped = rating.clamp(1, 5);
    // 1 sits at the left edge of its own stop rather than at 0%, so the
    // thumb is never off the end of the track.
    ((clamped - 1) as f64 / 4.0) * 100.0
}

/// Target-date horizons, in days from today. `None` = no deadline.
///
/// A horizon list rather than a date picker: "by when?" is a horizon
/// question far more often than a calendar one, and this keeps the
/// control a pill that matches the reference's language instead of a
/// native widget that looks like nothing else on the page.
const HORIZONS: [(&str, Option<i64>); 6] = [
    ("No date", None),
    ("1 week", Some(7)),
    ("1 month", Some(30)),
    ("3 months", Some(90)),
    ("6 months", Some(180)),
    ("1 year", Some(365)),
];

/// Resolves a horizon to a `YYYY-MM-DD` date, or `None` for no deadline.
///
/// Returns `None` rather than a bogus string when the arithmetic cannot be
/// represented: the store's `check_date` is shape-exact, and a malformed
/// date would fail the write after the call was already made.
fn horizon_date(days: Option<i64>) -> Option<String> {
    let d = days?;
    crate::app::date_plus_days(&crate::app::today_local(), d)
}

/// Splits raw compose text into a goal title and a description.
///
/// The title is the first non-empty line, clipped to 80 characters on a
/// word boundary. The 80 is not cosmetic: the manual path seeds the first
/// task with the goal title verbatim, so an unclipped 2000-character
/// ramble would become the task headline on the 420px canvas.
fn split_intent(raw: &str) -> (String, Option<String>) {
    const TITLE_MAX: usize = 80;
    let first = raw.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let trimmed = first.trim();
    let title = if trimmed.chars().count() <= TITLE_MAX {
        trimmed.to_string()
    } else {
        let clipped: String = trimmed.chars().take(TITLE_MAX).collect();
        match clipped.rfind(' ') {
            Some(i) if i > 0 => clipped[..i].trim_end().to_string(),
            _ => clipped,
        }
    };
    // Everything after the first non-empty line is the description. Blank
    // means `None` so the store writes NULL rather than an empty string.
    //
    // Written as an explicit walk rather than `skip_while`: the title is
    // the first *non-empty* line, so leading blanks must be skipped too —
    // and `skip_while(!is_empty)` does the exact opposite of what it looks
    // like (its predicate is true for non-empty lines, so it skips the
    // title and keeps the leading blanks).
    let mut rest: Vec<&str> = Vec::new();
    let mut past_title = false;
    for line in raw.lines() {
        if !past_title {
            if !line.trim().is_empty() {
                past_title = true;
            }
            continue;
        }
        rest.push(line);
    }
    let desc = rest.join("\n").trim().to_string();
    (title, if desc.is_empty() { None } else { Some(desc) })
}

/// Converts the shell's plan shape into the one the preview screens.
pub fn to_draft(preview: &serde_json::Value) -> Option<PlanDraft> {
    let plan = preview.get("plan")?;
    let milestones = plan
        .get("milestones")?
        .as_array()?
        .iter()
        .map(|m| PlanMilestone {
            title: m
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            rationale: m
                .get("rationale")
                .and_then(|v| v.as_str())
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string),
            steps: m
                .get("directives")
                .and_then(|v| v.as_array())
                .map(|ds| {
                    ds.iter()
                        .map(|d| PlanStep {
                            title: d
                                .get("title")
                                .and_then(|v| v.as_str())
                                .unwrap_or_default()
                                .to_string(),
                            instruction: d
                                .get("execution_context")
                                .and_then(|v| v.as_str())
                                .filter(|s| !s.trim().is_empty())
                                .map(str::to_string),
                            estimated_minutes: d
                                .get("estimated_minutes")
                                .and_then(|v| v.as_i64())
                                .unwrap_or(25),
                            after: d.get("after").and_then(|v| v.as_u64()).map(|n| n as usize),
                            phases: d
                                .get("phases")
                                .and_then(|v| v.as_array())
                                .map(|ps| {
                                    ps.iter()
                                        .map(|p| PlanPhase {
                                            title: p
                                                .get("title")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or_default()
                                                .to_string(),
                                            minutes: p
                                                .get("minutes")
                                                .and_then(|v| v.as_i64())
                                                .unwrap_or(0),
                                        })
                                        .collect()
                                })
                                .unwrap_or_default(),
                            edited: false,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        })
        .collect();
    Some(PlanDraft {
        title: plan
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        milestones,
        complexity: preview
            .get("complexity")
            .and_then(|v| v.as_i64())
            .unwrap_or(crate::domain::MAX_MINUTES.min(25))
            .clamp(1, 5),
        fallback: preview
            .get("fallback")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string),
        repair: preview
            .get("repair")
            .and_then(|r| r.get("notes"))
            .and_then(|v| v.as_array())
            .map(|ns| {
                ns.iter()
                    .filter_map(|n| n.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    })
}

#[component]
pub fn GoalCreateScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut intent = use_signal(String::new);
    let mut horizon = use_signal(|| "0usize".to_string());
    let mut complexity = use_signal(|| 3i64);
    let mut error = use_signal(|| None::<String>);
    // Cancel is a counter, not a flag: the fetch is still in flight after a
    // cancel, and the tick that finally returns has to recognise that its
    // result is no longer wanted. One boolean cannot express "a second
    // cancel is not a no-op".
    let mut cancel_epoch = use_signal(|| 0u32);

    let stage = *ctx.plan_stage.read();
    let busy = stage.is_busy();

    // Autofocus + auto-grow. `autofocus` on the textarea is unreliable
    // under a client-rendered tree — WebKit honours it during parsing, not
    // on an attribute set after insertion — so the field is focused
    // explicitly once the node exists. The growth, though, is not an
    // effect's job: the textarea node is stable across renders (Dioxus
    // diffs it in place; the busy state only rewrites the button labels),
    // so the height is re-measured from the event that can change it.
    use_effect(move || {
        crate::app::autofocus_compose();
        // The pre-flight is free and local, and re-running it on every
        // visit to this screen is what keeps the button honest: Settings can
        // change the model or the key while this page is unmounted.
        let ctx = ctx;
        spawn(async move {
            if let Ok(r) = invoke::<AiReadiness>("ai_readiness", ()).await {
                let mut ready = ctx.readiness;
                ready.set(Some(r));
            }
        });
    });

    let readiness = ctx.readiness.read().clone();
    let blocked = readiness.as_ref().and_then(|r| r.missing.clone());
    // The button's own gate, read once per render. A readiness that has
    // not loaded yet does NOT block Generate — refusing because we have
    // not asked yet would disable the button for every user whose first
    // paint beats the reply, and the shell re-checks everything anyway.
    let generate_blocked = readiness.as_ref().is_some_and(|r| !r.can_generate());

    let create_ai = move |_| {
        if busy {
            return;
        }
        let text = intent.read().clone();
        if text.trim().is_empty() {
            flash(&ctx, "DESCRIBE THE OBJECTIVE FIRST");
            return;
        }
        // Local refusals first, so a misconfiguration never costs a
        // request. The shell re-checks both — it is the authority — but
        // these are free and the message is already written.
        let why = ctx
            .readiness
            .read()
            .as_ref()
            .and_then(|r| r.missing.clone());
        if let Some(why) = why {
            flash(&ctx, &why);
            return;
        }
        let rating = *complexity.peek();
        let date = horizon_date(
            HORIZONS
                .get(horizon.peek().parse::<usize>().unwrap_or(0))
                .and_then(|(_, d)| *d),
        );
        let settings = ctx.settings.read().clone();
        let model = settings.tier1_model.clone().unwrap_or_default();
        if model.trim().is_empty() {
            flash(&ctx, "CHOOSE AN ARCHITECT MODEL IN SETTINGS");
            return;
        }
        {
            let mut s = ctx.plan_stage;
            s.set(PlanStage::Contacting);
        }
        cancel_epoch += 1;
        let epoch = *cancel_epoch.peek();
        let ctx = ctx;
        let cancel = cancel_epoch;
        spawn(async move {
            #[derive(serde::Serialize)]
            struct P {
                provider: String,
                model: String,
                /// Raw intent, NOT a title — the architect names the goal.
                intent: String,
                target_date: Option<String>,
                complexity: i64,
            }
            let req = P {
                provider: settings
                    .ai_provider
                    .clone()
                    .unwrap_or_else(|| "openrouter".into()),
                model,
                intent: text,
                target_date: date,
                complexity: rating,
            };
            match invoke::<serde_json::Value>("master_plan_preview", req).await {
                Ok(raw) => {
                    // Cancelled while the request was out: the reply is
                    // discarded and the button comes back. The write is
                    // never reached, which is the whole point of staging.
                    if *cancel.peek() != epoch {
                        let mut s = ctx.plan_stage;
                        s.set(PlanStage::Idle);
                        return;
                    }
                    match to_draft(&raw) {
                        Some(draft) => {
                            {
                                let mut p = ctx.plan;
                                p.set(Some(draft));
                            }
                            let mut s = ctx.plan_stage;
                            s.set(PlanStage::Idle);
                            let mut sc = ctx.screen;
                            *sc.write() = Screen::PlanPreview;
                        }
                        None => {
                            let mut s = ctx.plan_stage;
                            s.set(PlanStage::Idle);
                            *error.write() = Some(
                                "The plan came back in a shape this build cannot show.".into(),
                            );
                        }
                    }
                }
                // The provider's own words, and the shell has already
                // translated a network failure into "could not reach
                // <provider>". A bad key, a bad model id, an oversized
                // request and a genuine network failure are four different
                // problems, and "OFFLINE" told the user none of them.
                Err(e) => {
                    let mut s = ctx.plan_stage;
                    s.set(PlanStage::Idle);
                    if *cancel.peek() == epoch {
                        *error.write() = Some(e);
                    }
                }
            }
        });
    };

    let create_manual = move |_| {
        if busy {
            return;
        }
        let ctx = ctx;
        let raw = intent.read().clone();
        if raw.trim().is_empty() {
            flash(&ctx, "DESCRIBE THE OBJECTIVE FIRST");
            return;
        }
        let (title, description) = split_intent(&raw);
        if title.is_empty() {
            flash(&ctx, "DESCRIBE THE OBJECTIVE FIRST");
            return;
        }
        let date = horizon_date(
            HORIZONS
                .get(horizon.peek().parse::<usize>().unwrap_or(0))
                .and_then(|(_, d)| *d),
        );
        let rating = *complexity.peek();
        spawn(async move {
            #[derive(serde::Serialize)]
            struct G {
                title: String,
                description: Option<String>,
                target_date: Option<String>,
                complexity: i64,
            }
            let req = G {
                title,
                description,
                target_date: date,
                complexity: rating,
            };
            match invoke::<serde_json::Value>("create_goal", req).await {
                Ok(_) => {
                    flash(&ctx, "GOAL CREATED — FIRST TASK READY");
                    {
                        let mut s = ctx.screen;
                        *s.write() = Screen::Canvas;
                    }
                }
                Err(e) => flash(&ctx, &format!("ERR {e}")),
            }
        });
    };

    let cancel = move |_| {
        // Bumps the epoch the in-flight task compares against, so its
        // reply is dropped. The request itself is NOT cancelled — the
        // shell has no cancel channel and a request in flight is a request
        // already paid for — which is exactly why the epoch exists rather
        // than a flag the task would set when it finished.
        cancel_epoch += 1;
        let mut s = ctx.plan_stage;
        s.set(PlanStage::Idle);
        *error.write() = None;
    };

    let rating = *complexity.read();
    let word = complexity_word(rating);
    let fill = complexity_fill(rating);

    rsx! {
        div { class: "wl-page",
            // Edge controls on their own row, title centred beneath. See
            // the `wl-page-bar` note in wl.css for why these cannot share
            // a row with the title.
            div { class: "wl-page-bar",
                button {
                    class: "wl-circle-btn wl-back",
                    aria_label: "Back to the line",
                    title: "Back to the line",
                    onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::Canvas; } },
                    IconBack {}
                },
                // The last native `<select>` in the app. It is the one
                // that earns the exemption: a small closed control with no
                // catalog to search, no row to render, and — unlike the
                // provider select that used to sit in Settings — it has
                // never painted a white box. The provider needed a sheet
                // for its own reasons (a native widget cannot host the
                // model list, and could not be trusted to restyle at all).
                select {
                    class: "wl-horizon",
                    aria_label: "Target date",
                    title: "Target date",
                    value: "{horizon.read()}",
                    onchange: move |e| horizon.set(e.value()),
                    // Keyed like every other list in the app: the
                    // options are stable, so Dioxus should diff them by
                    // horizon rather than by position.
                    for (i, (label, _)) in HORIZONS.iter().enumerate() {
                        option { key: "{i}", value: "{i}", "{label}" }
                    }
                }
            }
            h1 { class: "wl-page-title wl-page-title--compose",
                "State the Objective"
            }

            // The field. `rows=1` plus the auto-grow on every input is what
            // makes it expand with the text instead of scrolling
            // internally.
            textarea {
                class: "wl-compose",
                rows: 1,
                maxlength: MAX_INTENT_CHARS,
                autofocus: true,
                spellcheck: true,
                placeholder: "What do you want to accomplish?",
                "aria-label": "Describe the objective in your own words",
                value: "{intent.read().clone()}",
                disabled: busy,
                oninput: move |e| {
                    // Client-side mirror of the shell's cap so the field
                    // cannot be typed past what `master_plan_preview`
                    // accepts.
                    let v: String = e.value().chars().take(MAX_INTENT_CHARS).collect();
                    intent.set(v);
                    // Editing the intent invalidates the last failure.
                    error.set(None);
                    // The height is measured from the DOM, not computed:
                    // wrapped line count cannot be derived from a
                    // font-size and a column width, and guessing produces
                    // a scrollbar on the second line.
                    crate::app::autogrow_compose();
                },
            }

            // Estimated Complexity. A five-stop track built from five
            // buttons rather than a native `range`: this project has
            // already shipped two native widgets WebKitGTK painted wrong,
            // and a slider is exactly the widget that cannot be restyled
            // from outside. Built this way it cannot be repainted, it takes
            // arrow keys, and it is announced properly.
            div { class: "wl-calibration",
                div { class: "wl-calibration-head",
                    span { class: "wl-calibration-label", "Estimated complexity" }
                    span { class: "wl-calibration-word", "{word}" }
                },
                div {
                    class: "wl-calibration-track",
                    // The filled prefix is the same cream as the progress
                    // bar, so "how far along" is one visual language rather
                    // than two.
                    div { class: "wl-calibration-fill", style: "width: {fill}%;" }
                    for i in 0..5i64 {
                        button {
                            key: "{i}",
                            class: "wl-calibration-stop",
                            class: if i + 1 == rating { "wl-calibration-stop--on" },
                            aria_pressed: if i + 1 == rating { "true" } else { "false" },
                            aria_label: complexity_word(i + 1),
                            title: complexity_word(i + 1),
                            disabled: busy,
                            onclick: move |_| complexity.set(i + 1),
                            span { class: "wl-calibration-dot" }
                        }
                    }
                },
                // The hint is load-bearing. A rating nobody moves is an
                // uninformative prior, so the control has to read as
                // something you only touch when it is obviously wrong —
                // not as a self-assessment to complete on every goal.
                p { class: "wl-hint",
                    "Leave it on standard unless the objective obviously is not a one-sitting thing."
                }
            }

            if let Some(msg) = error.read().clone() {
                p { class: "wl-form-error", "{msg}" }
            }
            // The pre-flight's own reason, shown as guidance rather than
            // as an error: nothing has failed, Generate is simply not
            // available yet and this is where to fix that.
            if let Some(why) = blocked.clone() {
                p { class: "wl-hint wl-calibration-blocked", "{why} — Generate is unavailable." }
            }

            if busy {
                // Two named stages, and both are real: this is the command
                // being awaited, not a timer. Progress events are not
                // available to this webview (the capability file grants no
                // permissions), so anything finer would be invented.
                p { class: "wl-form-error wl-plan-stage wl-mono", "{stage.label()}" }
            }

            div { class: "wl-actions-row",
                button { class: "wl-btn-primary", disabled: busy || generate_blocked,
                    onclick: create_ai,
                    if busy { "Working…" } else { "Generate" }
                }
                // The cancel is a ghost, not a danger button: it is not a
                // destructive act, it is a way out of a wait. A 75-second
                // budget with no exit is the complaint this whole flow was
                // rebuilt to fix, so the exit is as prominent as the wait.
                if busy {
                    button { class: "wl-btn-ghost", onclick: cancel, "Cancel" }
                } else {
                    // Was long enough to wrap a side-by-side row, so the
                    // no-API-key reassurance moved to the tooltip; the ghost
                    // styling already keeps it subordinate to Generate.
                    button { class: "wl-btn-ghost",
                        title: "Create manually — no API key required",
                        onclick: create_manual,
                        "Create manually"
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
    fn horizon_resolves_to_a_shape_exact_date() {
        // No deadline is `None`, not an empty string — the store's
        // `check_date` is shape-exact and would reject "".
        assert_eq!(horizon_date(None), None);
        let d = horizon_date(Some(7)).expect("a week is always representable");
        assert_eq!(d.len(), 10, "expected YYYY-MM-DD, got {d:?}");
        let bytes = d.as_bytes();
        assert!(
            bytes[4] == b'-' && bytes[7] == b'-',
            "bad separators in {d:?}"
        );
        assert!(
            bytes
                .iter()
                .enumerate()
                .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit()),
            "non-digit in {d:?}"
        );
    }

    /// The five words, in order, with the middle one being what an
    /// untouched slider shows. A default that is not the middle would be a
    /// prior that leans before the user has expressed an opinion.
    #[test]
    fn the_scale_is_five_words_and_defaults_to_the_middle() {
        assert_eq!(COMPLEXITY_STOPS.len(), 5);
        for (i, w) in COMPLEXITY_STOPS.iter().enumerate() {
            assert_eq!(complexity_word(i as i64 + 1), *w, "rating {}", i + 1);
        }
        assert_eq!(complexity_word(3), "standard");
        // Out of range clamps rather than panicking: the control cannot
        // produce one, and a fresh screen must still have a selection.
        assert_eq!(complexity_word(0), "light");
        assert_eq!(complexity_word(99), "deep");
    }

    /// The fill runs the length of the track, and neither end sits off it.
    /// A thumb at 0% or 100% renders half outside its own container, which
    /// is the kind of thing only shows up rendered.
    #[test]
    fn the_track_fills_end_to_end_without_running_off_it() {
        assert_eq!(complexity_fill(1), 0.0);
        assert_eq!(complexity_fill(3), 50.0);
        assert_eq!(complexity_fill(5), 100.0);
        for r in [1, 2, 3, 4, 5] {
            let f = complexity_fill(r);
            assert!((0.0..=100.0).contains(&f), "rating {r} filled {f}%");
        }
        // Clamped, like the word.
        assert_eq!(complexity_fill(0), 0.0);
        assert_eq!(complexity_fill(99), 100.0);
    }

    /// The manual path seeds the first task with the goal title verbatim,
    /// so an unclipped ramble would become the task headline on the 420px
    /// canvas. This is the guard for that.
    #[test]
    fn split_clips_the_title_to_a_card_safe_length() {
        let (title, desc) = split_intent("in n out burger\nthe rest of the plan");
        assert_eq!(title, "in n out burger");
        assert_eq!(desc.as_deref(), Some("the rest of the plan"));

        // Leading blank lines are skipped, not treated as the title.
        let (title, _) = split_intent("\n\n  ship it  \n");
        assert_eq!(title, "ship it");

        // A long single line clips on a word boundary.
        let long = format!("{} {}", "alpha ".repeat(60), "omega");
        let (title, _) = split_intent(&long);
        assert!(
            title.chars().count() <= 80,
            "got {} chars",
            title.chars().count()
        );
        assert!(title.ends_with("alpha"), "should clip on a word: {title:?}");

        // A single word longer than the cap must not clip to empty.
        let solid = "z".repeat(500);
        let (title, _) = split_intent(&solid);
        assert_eq!(title.chars().count(), 80);
        assert!(!title.is_empty());
    }

    #[test]
    fn split_reports_no_description_when_there_is_none() {
        let (title, desc) = split_intent("just a title");
        assert_eq!(title, "just a title");
        assert_eq!(desc, None, "blank remainder must be None, not Some(\"\")");
        assert_eq!(split_intent("   \n\t ").1, None);
    }

    /// The shell's preview shape becomes the screen's plan, and the rating
    /// survives the crossing — it is inside the payload, so a value the
    /// user chose cannot be lost between the fetch and the commit.
    #[test]
    fn the_shells_preview_becomes_a_draft_with_its_rating() {
        let raw = serde_json::json!({
            "plan": {
                "title": "Ship the relay",
                "milestones": [{
                    "title": "Wire it",
                    "rationale": "nothing is testable without it",
                    "directives": [
                        {"title": "Draft", "estimated_minutes": 20, "phases": []},
                        {"title": "Write", "estimated_minutes": 25, "phases": [],
                         "after": 0, "execution_context": "on paper"},
                    ],
                }],
            },
            "complexity": 5,
            "fallback": null,
            "repair": {"notes": ["phases were rescaled"]},
        });
        let d = to_draft(&raw).expect("a draft");
        assert_eq!(d.title, "Ship the relay");
        assert_eq!(d.complexity, 5);
        assert_eq!(d.step_count(), 2);
        assert_eq!(
            d.milestones[0].rationale.as_deref(),
            Some("nothing is testable without it")
        );
        let steps: Vec<&PlanStep> = d.steps().collect();
        assert_eq!(steps[0].after, None);
        assert_eq!(steps[1].after, Some(0));
        assert_eq!(steps[1].instruction.as_deref(), Some("on paper"));
        assert_eq!(d.repair, vec!["phases were rescaled"]);
        assert!(d.fallback.is_none());
    }

    /// The seeded stand-in says so. A silent swap would show a one-task
    /// plan and let the user believe the architect produced it.
    #[test]
    fn a_fallback_preview_keeps_its_reason() {
        let raw = serde_json::json!({
            "plan": {"title": "x", "milestones": []},
            "complexity": 4,
            "fallback": "The architect's plan could not be used",
            "repair": {"notes": []},
        });
        let d = to_draft(&raw).expect("a draft");
        assert!(d.fallback.as_deref().unwrap().contains("could not be used"));
        assert_eq!(d.complexity, 4);
    }

    #[test]
    fn a_preview_with_no_shape_is_refused_rather_than_shown_empty() {
        assert!(to_draft(&serde_json::json!({})).is_none());
        assert!(to_draft(&serde_json::json!({"plan": {}})).is_none());
    }

    /// An edit the shell would refuse is refused where it is typed, and the
    /// draft is left alone. A bad number arriving as a failed COMMIT —
    /// after the user approved a preview — is the failure this prevents.
    #[test]
    fn an_edit_the_store_would_refuse_is_refused_in_place() {
        let mut d = PlanDraft {
            title: "T".into(),
            milestones: vec![PlanMilestone {
                title: "M".into(),
                rationale: None,
                steps: vec![PlanStep {
                    title: "a".into(),
                    estimated_minutes: 20,
                    ..Default::default()
                }],
            }],
            complexity: 3,
            ..Default::default()
        };
        assert!(d.set_step_minutes(0, 45).is_ok());
        assert_eq!(d.steps().next().unwrap().estimated_minutes, 45);
        for bad in [0, -5, crate::domain::MAX_MINUTES + 1] {
            assert!(d.set_step_minutes(0, bad).is_err(), "{bad}");
        }
        assert_eq!(
            d.steps().next().unwrap().estimated_minutes,
            45,
            "a refused edit must not have been applied"
        );
        // An out-of-range index is a no-op, not a panic: the index comes
        // from an edited payload and a preview that crashes is worse.
        assert!(d.set_step_minutes(9, 30).is_ok());
        d.rename_step(9, "x");
        assert_eq!(
            d.steps().next().unwrap().title,
            "a",
            "a rename past the end must not have touched anything"
        );
    }

    /// Re-parenting refuses an edge the committed plan would not have, so
    /// the graph on screen and the graph on the line agree.
    #[test]
    fn an_edge_to_a_later_task_is_refused() {
        let mut d = PlanDraft {
            milestones: vec![PlanMilestone {
                steps: vec![PlanStep::default(), PlanStep::default()],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(d.set_after(0, None).is_ok());
        assert!(d.set_after(1, Some(0)).is_ok());
        assert!(d.set_after(0, Some(1)).is_err(), "forward");
        assert!(d.set_after(1, Some(1)).is_err(), "self");
        assert!(
            d.set_after(1, None).is_ok(),
            "clearing an edge is always allowed"
        );
        assert_eq!(
            d.milestones[0].steps[1].after, None,
            "and it must actually clear, so a task can be unblocked from the preview"
        );
    }

    /// The stage line is the only account a user has of a 75-second
    /// budget, and the two words are the two commands actually awaited.
    #[test]
    fn the_two_stages_name_the_commands_being_awaited() {
        assert_eq!(PlanStage::Idle.label(), "Generate");
        assert!(!PlanStage::Idle.is_busy());
        assert_eq!(PlanStage::Contacting.label(), "Contacting the architect…");
        assert_eq!(PlanStage::Writing.label(), "Writing the plan…");
        assert!(PlanStage::Contacting.is_busy() && PlanStage::Writing.is_busy());
    }
}
