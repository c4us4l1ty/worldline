//! Morning Brief — Doppio One editorial greeting + Tier-2 dispatch
//! (or the manual fallback when no BYOK key is configured).
//!
//! B-005 contract: the shell returns `{titles, created_ids}` — authored
//! vs PERSISTED directives. After a successful briefing the screen
//! re-fetches `current_directive` + `velocity` so the canvas and HUD
//! reflect reality; zero persisted directives shows an explicit
//! empty-state CTA to goal creation instead of a blank canvas.

use dioxus::prelude::*;

use crate::app::{
    flash, invoke, record_sync, set_directive, AppCtx, BriefingView, DirectiveView, Screen,
    SyncStats, VelocityView,
};

pub fn MorningBriefScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut constraints = use_signal(String::new);
    let mut briefing = use_signal::<Option<BriefingView>>(|| None);
    let mut busy = use_signal(|| false);
    // Provider failures render inline (see the note in `goal_create.rs`):
    // a real diagnosis must be readable, and the toast is a pill.
    let mut error = use_signal(|| None::<String>);

    let mut fetch_brief = move || {
        let ctx = ctx;
        let constraints = constraints.read().clone();
        let provider = ctx.settings.read().ai_provider.clone();
        let tier2 = ctx.settings.read().tier2_model.clone();
        // No model, no call — same contract as the compose screen. This
        // used to send the placeholder `"haiku-class"`, which is not a
        // model on any provider, so the request 400'd and the screen
        // reported "OFFLINE — MANUAL MODE".
        let Some(model) = tier2.filter(|m| !m.trim().is_empty()) else {
            error.set(Some(
                "No dispatcher model selected — choose one in Settings.".to_string(),
            ));
            return;
        };
        error.set(None);
        busy.set(true);
        spawn(async move {
            // BYOK dispatch (keys injected from the vault by the shell;
            // in v0.1 the shell reads provider config from settings).
            #[derive(serde::Serialize)]
            struct B {
                provider: String,
                model: String,
                constraints: String,
            }
            let req = B {
                provider: provider.clone().unwrap_or_else(|| "openrouter".into()),
                model,
                constraints,
            };
            match invoke::<BriefingView>("morning_briefing", req).await {
                Ok(brief) => {
                    // Truth first: surface persisted count before any
                    // navigation assumption (B-005).
                    if brief.created_ids.is_empty() {
                        flash(&ctx, "BRIEFING NOT PERSISTED — NO ACTIVE DIRECTIVE");
                    } else {
                        flash(&ctx, "BRIEFING COMPILED");
                    }
                    // Re-fetch the real directive + velocity so Canvas
                    // never renders stale state after this screen.
                    match invoke::<Option<DirectiveView>>("current_directive", ()).await {
                        Ok(dv) => set_directive(&ctx, dv),
                        Err(e) => flash(&ctx, &format!("ERR {e}")),
                    }
                    if let Ok(v) = invoke::<VelocityView>("velocity", ()).await {
                        {
                            let mut s = ctx.velocity;
                            *s.write() = v;
                        }
                    }
                    // Sync surfacing (B-005/MVP-4): show what the
                    // briefing left in the outbox / last cycle. NoRelay
                    // and offline are expected here — LOCAL is the
                    // honest pill, not an error flash.
                    match invoke::<SyncStats>("sync_now", ()).await {
                        Ok(s) => {
                            record_sync(&ctx, &s);
                            if s.pushed > 0 || s.pulled > 0 {
                                flash(&ctx, s.summary().as_str());
                            }
                        }
                        Err(_) => {
                            let mut st = ctx.sync_status;
                            *st.write() = "LOCAL".to_string();
                        }
                    }
                    briefing.set(Some(brief));
                }
                Err(e) => {
                    // A missing key is a configuration state and a bad
                    // model is a typo; both now say which. Everything
                    // else arrives already phrased by the shell from the
                    // provider's own response body.
                    if e.contains("no API key") {
                        *error.write() = Some(format!(
                            "No API key stored for {}. Add one in Settings, or plan manually.",
                            provider.unwrap_or_default()
                        ));
                    } else {
                        *error.write() = Some(e);
                    }
                }
            }
            busy.set(false);
        });
    };

    let hour = chrono::Local::now().format("%H").to_string();
    let greeting = match hour.parse::<u32>() {
        Ok(h) if h < 12 => "morning",
        Ok(h) if h < 18 => "afternoon",
        _ => "evening",
    };
    let brief = briefing.read().clone();
    let brief_count = brief.as_ref().map(|b| b.titles.len().min(3)).unwrap_or(0);
    let persisted = brief.as_ref().map(|b| b.created_ids.len()).unwrap_or(0);

    rsx! {
        div {
            class: "wl-scroll-region",
            h1 { class: "wl-brief-greeting",
                "Good {greeting}. Today's " span { class: "wl-italic-accent", "trajectory" } "."
            }
            p { class: "wl-body-muted", style: "margin: 12px 0 18px;",
                "One to three non-negotiable directives. The dispatcher sees only the active milestone, the last 48 hours of velocity, and your constraints."
            }

            if briefing.read().is_none() {
                div { class: "wl-field",
                    label { class: "wl-label", "Constraints for today" }
                    textarea {
                        class: "wl-textarea",
                        placeholder: "e.g. 2 focused hours, meetings until noon…",
                        value: "{constraints.read().clone()}",
                        oninput: move |e| constraints.set(e.value()),
                    }
                }
                button { class: "wl-btn-primary", disabled: *busy.read(),
                    onclick: move |_| fetch_brief(),
                    if *busy.read() { "Dispatching…" } else { "Generate briefing" }
                }
                if let Some(msg) = error.read().clone() {
                    p { class: "wl-form-error", "{msg}" }
                }
                button {
                    class: "wl-btn-escape",
                    style: "margin-top: 6px;",
                    onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::GoalCreate; } },
                    "No API key — plan manually"
                }
            } else if persisted == 0 {
                // B-005: authored titles exist but nothing landed in the
                // store — never navigate to a canvas that would show
                // stale/empty data silently.
                p { class: "wl-body-muted", style: "margin-bottom: 12px;",
                    "The dispatcher drafted {brief_count} directive(s) but none could be saved — no active milestone, or the session could not write. Create a goal first, or check settings."
                }
                button {
                    class: "wl-btn-primary",
                    onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::GoalCreate; } },
                    "Create a goal"
                }
                button {
                    class: "wl-btn-ghost",
                    style: "margin-top: 8px;",
                    onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::Settings; } },
                    "Open settings"
                }
            } else {
                p { class: "wl-body-muted wl-mono", style: "margin-bottom: 12px;",
                    "{persisted} of {brief_count} drafted directive(s) on the line — estimates render on the canvas."
                }
                div { class: "wl-directive-card", style: "margin-bottom: 16px;",
                    for (i, d) in brief.as_ref().unwrap().titles.iter().take(3).enumerate() {
                        div { key: "{i}", style: "margin-bottom: 12px;",
                            div { class: "wl-directive-step-badge", "[0{i + 1}] Dispatched directive" }
                            p { class: "wl-directive-title", style: "font-size: 19px; margin-bottom: 4px;", "{d}" }
                            p { class: "wl-body-muted", "Est: on canvas · Stackelberg order {i + 1}/{brief_count}" }
                        }
                    }
                }
                p { class: "wl-body-muted", style: "margin-bottom: 12px;",
                    "Eliminate decision overhead. Follow the vector."
                }
                button {
                    class: "wl-btn-primary",
                    onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::Canvas; } },
                    span { "Commence Directive 01" }
                    kbd { class: "wl-kbd", "⌘↵" }
                }
            }
        }
    }
}
