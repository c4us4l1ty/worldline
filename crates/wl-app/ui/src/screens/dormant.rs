//! Dormant state (spec FSM: after Evening Audit, until next milestone).
//! Zero-guilt rest screen — no lists, no streaks.

use dioxus::prelude::*;

use crate::app::{AppCtx, Screen};

pub fn DormantScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    rsx! {
        div { class: "wl-directive-container",
            div { class: "wl-directive-card",
                div { class: "wl-directive-step-badge", "Dormant · until next milestone" }
                h1 { class: "wl-serif-title",
                    "Rest. The " span { class: "wl-italic-accent", "line" } " holds."
                }
                p { class: "wl-body-muted",
                    "Velocity recalibrated. No debt carried forward — tomorrow starts from a clean vector."
                }
                div { style: "display: flex; flex-direction: column; gap: 8px; margin-top: 18px;",
                    button {
                        class: "wl-btn-primary",
                        onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::MorningBrief; } },
                        "Morning briefing"
                    }
                    button {
                        class: "wl-btn-escape",
                        onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::Canvas; } },
                        "Return to the line"
                    }
                }
            }
        }
    }
}
