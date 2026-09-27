//! Goal creation — the compose screen.
//!
//! One free-text field owns the whole page. The user types intent in
//! their own words ("in n out burger"); the architect names the goal and
//! builds the milestone hierarchy. The manual path uses the same text
//! directly and needs no API key (PRD manual-fallback decision).
//!
//! The screen deliberately borrows the canvas's premise — ONE thing, no
//! chrome — rather than the settings page's four-cards-and-a-save shape.
//! There is no title field, no details field, and no constraints field
//! anymore: `wl-core`'s `fallback_title` derives a name from the first
//! line when the architect does not offer one, and the Tier-1 prompt is
//! told to read the whole text as intent.

use dioxus::prelude::*;

use crate::app::{flash, invoke, AppCtx, Screen};
use crate::icons::IconBack;

/// Hard cap on the compose field. Matches `domain::MAX_DESCRIPTION_CHARS`
/// so the shell accepts anything the UI lets you type — a client-side cap
/// that disagreed with the store's would either block valid prose or let a
/// request through that fails at the write boundary after the BYOK call.
const MAX_INTENT_CHARS: usize = 4000;

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
/// directive with the goal title verbatim, so an unclipped 2000-character
/// ramble would become the directive headline on the 420px canvas.
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

#[component]
pub fn GoalCreateScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut intent = use_signal(String::new);
    let mut horizon = use_signal(|| "0usize".to_string());
    let mut busy = use_signal(|| false);

    // Autofocus + auto-grow. `autofocus` on the textarea handles the first
    // paint; this effect re-asserts both on every intent change, and on
    // mount, because the field is re-created when the busy state flips the
    // button labels — which drops focus and collapses the height back to
    // the `rows=1` box.
    use_effect(move || {
        // Read the signal so the effect re-runs per keystroke: the height
        // has to follow the text, and `scrollHeight` is the only way to
        // know how many lines that is.
        let _text = intent.read().clone();
        spawn(async move {
            crate::app::autofocus_compose();
            crate::app::autogrow_compose();
        });
    });

    let mut create_ai = move || {
        if *busy.peek() {
            return;
        }
        let text = intent.read().clone();
        if text.trim().is_empty() {
            flash(&ctx, "DESCRIBE THE OBJECTIVE FIRST");
            return;
        }
        let ctx = ctx;
        let settings = ctx.settings.read().clone();
        let date = horizon_date(
            HORIZONS
                .get(horizon.peek().parse::<usize>().unwrap_or(0))
                .and_then(|(_, d)| *d),
        );
        busy.set(true);
        spawn(async move {
            #[derive(serde::Serialize)]
            struct P {
                provider: String,
                model: String,
                /// Raw intent, NOT a title — the architect names the goal.
                intent: String,
                target_date: Option<String>,
            }
            let req = P {
                provider: settings
                    .ai_provider
                    .clone()
                    .unwrap_or_else(|| "openrouter".into()),
                model: settings
                    .tier1_model
                    .clone()
                    .unwrap_or_else(|| "flagship".into()),
                intent: text,
                target_date: date,
            };
            match invoke::<String>("master_plan", req).await {
                Ok(_goal_id) => {
                    flash(&ctx, "MASTER PLAN COMPILED");
                    {
                        let mut s = ctx.screen;
                        *s.write() = Screen::Canvas;
                    }
                }
                Err(_) => flash(&ctx, "OFFLINE — USE MANUAL MODE"),
            }
            busy.set(false);
        });
    };

    let mut create_manual = move || {
        if *busy.peek() {
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
        busy.set(true);
        spawn(async move {
            #[derive(serde::Serialize)]
            struct G {
                title: String,
                description: Option<String>,
                target_date: Option<String>,
            }
            let req = G {
                title,
                description,
                target_date: date,
            };
            match invoke::<serde_json::Value>("create_goal", req).await {
                Ok(_) => {
                    flash(&ctx, "GOAL CREATED — FIRST DIRECTIVE READY");
                    {
                        let mut s = ctx.screen;
                        *s.write() = Screen::Canvas;
                    }
                }
                Err(e) => {
                    flash(&ctx, &format!("ERR {e}"));
                    busy.set(false);
                }
            }
        });
    };

    rsx! {
        div { class: "wl-page",
            // Edge controls on their own row, title centred beneath. See
            // the `wl-page-bar` note in wl.css for why these cannot share
            // a row with the title.
            div { class: "wl-page-bar",
                button {
                    class: "wl-back",
                    aria_label: "Back to the line",
                    title: "Back to the line",
                    onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::Canvas; } },
                    IconBack {}
                }
                select {
                    class: "wl-horizon",
                    aria_label: "Target date",
                    title: "Target date",
                    value: "{horizon.read()}",
                    onchange: move |e| horizon.set(e.value()),
                    for (i, (label, _)) in HORIZONS.iter().enumerate() {
                        option { value: "{i}", "{label}" }
                    }
                }
            }
            h1 { class: "wl-page-title wl-page-title--compose",
                "State the Objective"
            }

            // The field. `rows=1` plus the auto-grow effect is what makes
            // it expand with the text instead of scrolling internally.
            textarea {
                class: "wl-compose",
                rows: 1,
                maxlength: MAX_INTENT_CHARS,
                autofocus: true,
                spellcheck: true,
                placeholder: "What do you want to accomplish?",
                "aria-label": "Describe the objective in your own words",
                value: "{intent.read().clone()}",
                oninput: move |e| {
                    // Client-side mirror of the shell's cap so the field
                    // cannot be typed past what `master_plan` accepts.
                    let v: String = e.value().chars().take(MAX_INTENT_CHARS).collect();
                    intent.set(v);
                },
            }

            div { class: "wl-actions-row",
                button { class: "wl-btn-primary", disabled: *busy.read(),
                    onclick: move |_| create_ai(),
                    if *busy.read() { "Planning…" } else { "Generate" }
                }
                // Was long enough to wrap a side-by-side row, so the
                // no-API-key reassurance moved to the tooltip; the ghost
                // styling already keeps it subordinate to Generate.
                button { class: "wl-btn-ghost", disabled: *busy.read(),
                    title: "Create manually — no API key required",
                    onclick: move |_| create_manual(),
                    if *busy.read() { "Creating…" } else { "Create manually" }
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

    /// The manual path seeds the first directive with the goal title
    /// verbatim, so an unclipped ramble would become the directive
    /// headline on the 420px canvas. This is the guard for that.
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
}
