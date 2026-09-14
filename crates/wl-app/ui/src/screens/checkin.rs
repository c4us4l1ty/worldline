//! Evening Check-In — the 30-second objective audit (PRD §5.4).
//! Done / Partial / Skipped. Zero-guilt GPS recalibration display.

use dioxus::prelude::*;

use crate::app::{flash, invoke, AppCtx, Screen, VelocityView};

pub fn CheckInScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut note = use_signal(String::new);
    let mut submitted = use_signal(|| false);

    let record = move |outcome: &str| {
        let ctx = ctx;
        let note = note.read().clone();
        let outcome = outcome.to_string();
        spawn(async move {
            #[derive(serde::Serialize)]
            struct C {
                outcome: String,
                note: Option<String>,
            }
            let req = C {
                outcome,
                note: if note.trim().is_empty() {
                    None
                } else {
                    Some(note)
                },
            };
            match invoke::<VelocityView>("check_in", req).await {
                Ok(v) => {
                    {
                        let mut s = ctx.velocity;
                        *s.write() = v;
                    }
                    submitted.set(true);
                    flash(&ctx, "LOGGED — OBJECTIVELY");
                }
                Err(e) => flash(&ctx, &format!("ERR {e}")),
            }
        });
    };

    let v = ctx.velocity.read().clone();
    let pct = (v.estimate_adjustment * 100.0).round();

    rsx! {
        div { class: "wl-scroll-region",
            h1 { class: "wl-serif-title",
                "Evening " span { class: "wl-italic-accent", "audit" }
            }
            p { class: "wl-body-muted", style: "margin-bottom: 18px;",
                "Thirty seconds. One honest answer. Skips are velocity adjustments, not failures — the system recalibrates like a GPS, never like an alarm."
            }

            if !*submitted.read() {
                div { class: "wl-field",
                    textarea {
                        class: "wl-textarea",
                        placeholder: "Optional: what happened today?",
                        value: "{note.read().clone()}",
                        oninput: move |e| note.set(e.value()),
                    }
                }
                div { class: "wl-checkin-row",
                    button { class: "wl-checkin-choice", onclick: move |_| record("done"),
                        span { class: "wl-checkin-glyph", "●" }
                        span { class: "wl-checkin-label", "Done" }
                    }
                    button { class: "wl-checkin-choice", onclick: move |_| record("partial"),
                        span { class: "wl-checkin-glyph", "◐" }
                        span { class: "wl-checkin-label", "Partial" }
                    }
                    button { class: "wl-checkin-choice", onclick: move |_| record("skipped"),
                        span { class: "wl-checkin-glyph", "○" }
                        span { class: "wl-checkin-label", "Skipped" }
                    }
                }
            } else {
                div { class: "wl-velocity-strip",
                    div {
                        span { class: "wl-velocity-metric", "Milestones left" }
                        span { class: "wl-velocity-value", "{v.milestones_remaining}" }
                    }
                    div {
                        span { class: "wl-velocity-metric", "Days left" }
                        span { class: "wl-velocity-value", "{v.days_remaining}" }
                    }
                    div {
                        span { class: "wl-velocity-metric", "Recalibrated" }
                        span { class: "wl-velocity-value", "{pct}%" }
                    }
                }
                p { class: "wl-body-muted", style: "margin-top: 16px;",
                    "Trajectory adjusted for reality. Tomorrow's estimates reflect actual velocity — no backlog guilt."
                }
                button {
                    class: "wl-btn-ghost",
                    style: "margin-top: 14px;",
                    onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::Canvas; } },
                    "Return to the line"
                }
            }
        }
    }
}
