//! Morning Brief — Doppio One editorial greeting + Tier-2 dispatch
//! (or the manual fallback when no BYOK key is configured).

use dioxus::prelude::*;

use crate::app::{flash, invoke, AppCtx, Screen};

pub fn MorningBriefScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut constraints = use_signal(String::new);
    let mut briefing = use_signal::<Option<Vec<String>>>(|| None);
    let mut busy = use_signal(|| false);

    let mut fetch_brief = move || {
        let ctx = ctx;
        let constraints = constraints.read().clone();
        let provider = ctx.settings.read().ai_provider.clone();
        let tier2 = ctx.settings.read().tier2_model.clone();
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
                model: tier2.unwrap_or_else(|| "haiku-class".into()),
                constraints,
            };
            match invoke::<Vec<String>>("morning_briefing", req).await {
                Ok(directives) => {
                    briefing.set(Some(directives));
                    flash(&ctx, "BRIEFING COMPILED");
                }
                Err(e) => {
                    // Missing BYOK key is a configuration state, not a
                    // network failure — say so instead of crying offline.
                    if e.contains("no API key") {
                        flash(&ctx, "NO API KEY — ADD ONE IN SETTINGS OR PLAN MANUALLY");
                    } else {
                        flash(&ctx, "OFFLINE — MANUAL MODE");
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
    let brief_count = briefing.read().clone().unwrap_or_default().len().min(3);

    rsx! {
        div { class: "wl-scroll-region",
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
                button {
                    class: "wl-btn-escape",
                    style: "margin-top: 6px;",
                    onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::GoalCreate; } },
                    "No API key — plan manually"
                }
            } else {
                p { class: "wl-body-muted wl-mono", style: "margin-bottom: 12px;",
                    "Shell dispatch returns titles only — estimates render on the canvas."
                }
                div { class: "wl-directive-card", style: "margin-bottom: 16px;",
                    for (i, d) in briefing.read().clone().unwrap_or_default().iter().take(3).enumerate() {
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
