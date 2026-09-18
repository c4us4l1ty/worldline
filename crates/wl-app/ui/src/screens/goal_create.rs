//! Goal Creation — Tier-1 Master Architect (BYOK) or manual fallback
//! (no API key required, PRD manual-fallback decision).

use dioxus::prelude::*;

use crate::app::{flash, invoke, AppCtx, Screen};

#[component]
pub fn GoalCreateScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut title = use_signal(String::new);
    let mut description = use_signal(String::new);
    let mut target_date = use_signal(String::new);
    let mut context = use_signal(String::new);
    let mut busy = use_signal(|| false);

    let mut create_ai = move || {
        if *busy.peek() {
            return;
        }
        let ctx = ctx;
        let payload = (
            title.read().clone(),
            description.read().clone(),
            target_date.read().clone(),
            context.read().clone(),
        );
        if payload.0.trim().is_empty() {
            flash(&ctx, "TITLE REQUIRED");
            return;
        }
        let settings = ctx.settings.read().clone();
        busy.set(true);
        spawn(async move {
            #[derive(serde::Serialize)]
            struct P {
                provider: String,
                model: String,
                goal_title: String,
                goal_description: Option<String>,
                target_date: Option<String>,
                context: String,
            }
            let req = P {
                provider: settings
                    .ai_provider
                    .clone()
                    .unwrap_or_else(|| "openai-compat".into()),
                model: settings
                    .tier1_model
                    .clone()
                    .unwrap_or_else(|| "flagship".into()),
                goal_title: payload.0,
                goal_description: if payload.1.trim().is_empty() {
                    None
                } else {
                    Some(payload.1)
                },
                target_date: if payload.2.trim().is_empty() {
                    None
                } else {
                    Some(payload.2)
                },
                context: payload.3,
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

    let create_manual = move || {
        if *busy.peek() {
            return;
        }
        let ctx = ctx;
        let title = title.read().clone();
        let description = description.read().clone();
        let target_date = target_date.read().clone();
        if title.trim().is_empty() {
            flash(&ctx, "TITLE REQUIRED");
            return;
        }
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
                description: if description.trim().is_empty() {
                    None
                } else {
                    Some(description)
                },
                target_date: if target_date.trim().is_empty() {
                    None
                } else {
                    Some(target_date)
                },
            };
            match invoke::<serde_json::Value>("create_goal", req).await {
                Ok(_) => {
                    flash(&ctx, "GOAL CREATED — ADD MILESTONES");
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
        div { class: "wl-scroll-region",
            h1 { class: "wl-serif-title",
                "State the " span { class: "wl-italic-accent", "objective" }
            }
            p { class: "wl-body-muted", style: "margin: 10px 0 18px;",
                "The architect builds the milestone hierarchy; you execute one directive at a time. Manual mode needs no API key — the local engine handles the rest."
            }

            div { class: "wl-field",
                label { class: "wl-label", "Goal" }
                input { class: "wl-input", r#type: "text", placeholder: "e.g. Ship Worldline v0.1",
                    value: "{title.read().clone()}",
                    oninput: move |e| title.set(e.value()) }
            }
            div { class: "wl-field",
                label { class: "wl-label", "Details" }
                textarea { class: "wl-textarea", placeholder: "Optional context for the architect",
                    value: "{description.read().clone()}",
                    oninput: move |e| description.set(e.value()) }
            }
            div { class: "wl-field",
                label { class: "wl-label", "Target date (YYYY-MM-DD)" }
                input { class: "wl-input", r#type: "date",
                    value: "{target_date.read().clone()}",
                    oninput: move |e| target_date.set(e.value()) }
            }
            div { class: "wl-field",
                label { class: "wl-label", "Constraints (for Tier-1 architect)" }
                textarea { class: "wl-textarea", placeholder: "Hours/day, skills, hard deadlines…",
                    value: "{context.read().clone()}",
                    oninput: move |e| context.set(e.value()) }
            }

            button { class: "wl-btn-primary", disabled: *busy.read(),
                onclick: move |_| create_ai(),
                if *busy.read() { "Architecting…" } else { "Generate master plan (Tier-1)" }
            }
            button { class: "wl-btn-escape", style: "margin-top: 6px;", disabled: *busy.read(),
                onclick: move |_| create_manual(),
                "Create manually — no API key"
            }
        }
    }
}
