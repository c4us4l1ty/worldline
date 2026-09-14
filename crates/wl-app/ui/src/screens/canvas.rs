//! The Stackelberg Single-Directive Canvas (skill §4.A/B/C, US-3).
//!
//! Renders exactly ONE directive. ⌘+Enter completes; Escape opens the
//! frictionful escape hatch modal requiring categorization.

use dioxus::prelude::*;

use crate::app::{flash, fmt_mmss, invoke, AppCtx, DirectiveView};

pub fn CanvasScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    let directive = use_signal::<Option<DirectiveView>>(|| None);
    let escape_reason_note = use_signal(String::new);

    // Load the current directive on mount.
    use_effect(move || {
        let mut d = directive;
        let ctx_loader = ctx;
        spawn(async move {
            match invoke::<DirectiveView>("current_directive", ()).await {
                Ok(dv) => {
                    *d.write() = Some(dv);
                }
                Err(e) => flash(&ctx_loader, &format!("ERR {e}")),
            }
        });
    });

    let d = directive.read().clone();
    let timer = fmt_mmss(*ctx.timer_secs.read());
    let escaping = *ctx.escape_open.read();
    let milestone_label = d
        .as_ref()
        .and_then(|x| x.milestone_title.clone())
        .unwrap_or_else(|| "No milestone".to_string());
    let hour = chrono::Local::now().format("%H").to_string();
    let evening = hour.parse::<u32>().map(|h| h >= 19).unwrap_or(false);

    rsx! {
        div {
            class: "wl-root",
            tabindex: "0",
            autofocus: "true",
            onkeydown: move |e: Event<KeyboardData>| {
                // ⌘+Enter / Ctrl+Enter completes; Escape toggles bailout (skill §7.5).
                if e.key() == Key::Enter && (e.modifiers().meta() || e.modifiers().ctrl()) {
                    let ctx = ctx;
                    spawn(async move { complete_current(&ctx).await; });
                } else if e.key() == Key::Escape {
                    let open = *ctx.escape_open.read();
                    { let mut s = ctx.escape_open; *s.write() = !open; }
                }
            },
            style: "display: flex; flex-direction: column; flex: 1; min-height: 0; outline: none;",
        // HUD (skill §4.A)
        header { class: "wl-hud",
            div { class: "wl-hud-meta",
                if evening {
                    button {
                        class: "wl-hud-pill",
                        style: "border: none; cursor: pointer;",
                        onclick: move |_| { { let mut s = ctx.screen; *s.write() = crate::app::Screen::EveningCheckIn; } },
                        "Evening audit"
                    }
                }
                span { class: "wl-hud-pill", "{milestone_label}" }
            }
            div { style: "display: flex; gap: 8px; align-items: center;",
                span { class: "wl-hud-status",
                    span { class: "wl-pulse-dot" }
                    "{ctx.sync_status.read().clone()}"
                }
                button {
                    class: "wl-hud-timer",
                    style: "border: 1px solid var(--wl-border-subtle); cursor: pointer; font-family: var(--font-mono);",
                    onclick: move |_| { { let mut s = ctx.screen; *s.write() = crate::app::Screen::Settings; } },
                    "{timer}"
                }
            }
        }

        // Directive card (skill §4.B) — the ONLY directive.
        main { class: "wl-directive-container",
            if let Some(d) = d {
                DirectiveCard { d: d }
            } else {
                div { class: "wl-directive-card",
                    h1 { class: "wl-serif-title", "The line is clear" }
                    p { class: "wl-body-muted",
                        "No directive is active. Let the architect plan your trajectory."
                    }
                    div { style: "display: flex; gap: 8px; margin-top: 18px;",
                        button { class: "wl-btn-ghost", onclick: move |_| { { let mut s = ctx.screen; *s.write() = crate::app::Screen::MorningBrief; } }, "Morning briefing" }
                    }
                    div { style: "display: flex; gap: 8px; margin-top: 8px;",
                        button { class: "wl-btn-ghost", onclick: move |_| { { let mut s = ctx.screen; *s.write() = crate::app::Screen::GoalCreate; } }, "Create a goal" }
                    }
                }
            }
        }

        // Action controls (skill §4.C)
        footer { class: "wl-actions",
            button { class: "wl-btn-primary",
                onclick: move |_| {
                    let ctx = ctx;
                    spawn(async move { complete_current(&ctx).await; });
                },
                span { "Complete Directive" }
                kbd { class: "wl-kbd", "⌘↵" }
            }
            button { class: "wl-btn-escape",
                onclick: move |_| { { let mut s = ctx.escape_open; *s.write() = true; } },
                span { "Bailout / Blocked" }
                kbd { class: "wl-kbd-subtle", "Esc" }
            }
        }

        // Frictionful escape hatch modal (skill §4.C, §5 "Do")
        if escaping {
            EscapeModal { note: escape_reason_note }
        }
        }
    }
}

#[component]
pub fn DirectiveCard(d: DirectiveView) -> Element {
    let phase_badge = match d.phase {
        Some((step, total)) => format!("Phase {step} of {total}"),
        None => format!("{} Minutes", d.estimated_minutes.max(0)),
    };
    let progress = match d.phase {
        Some((step, total)) if total > 0 => (step as f64 / total as f64) * 100.0,
        _ => 100.0,
    };
    rsx! {
        div { class: "wl-directive-card",
            div { class: "wl-directive-step-badge", "{phase_badge}" }
            h1 { class: "wl-directive-title", "{d.title}" }
            if let Some(instr) = &d.instruction {
                p { class: "wl-directive-instruction", "{instr}" }
            }
            div { class: "wl-progress-track",
                div { class: "wl-progress-bar", style: "width: {progress}%;" }
            }
        }
    }
}

#[component]
fn EscapeModal(note: Signal<String>) -> Element {
    let ctx = use_context::<AppCtx>();
    let mut confirming = use_signal(|| false);

    let bail = move |reason: &str| {
        let ctx = ctx;
        let note = note.read().clone();
        let reason = reason.to_string();
        spawn(async move {
            #[derive(serde::Serialize)]
            struct BailReq {
                reason: String,
                note: Option<String>,
            }
            let req = BailReq {
                reason,
                note: if note.trim().is_empty() {
                    None
                } else {
                    Some(note)
                },
            };
            match invoke::<serde_json::Value>("bail_out", req).await {
                Ok(_) => {
                    let mut escape = ctx.escape_open;
                    *escape.write() = false;
                    flash(&ctx, "VELOCITY ADJUSTED");
                    let mut d = ctx.directive;
                    match invoke::<DirectiveView>("current_directive", ()).await {
                        Ok(dv) => *d.write() = Some(dv),
                        Err(_) => *d.write() = None,
                    }
                }
                Err(e) => flash(&ctx, &format!("ERR {e}")),
            }
            confirming.set(false);
        });
    };

    rsx! {
        div { class: "wl-modal-backdrop",
            onclick: move |_| { confirming.set(true); /* backdrop click = confirm-dismiss prompt */ },
            div { class: "wl-modal-sheet", onclick: move |e| e.stop_propagation(),
                if !*confirming.read() {
                    h2 { class: "wl-modal-header", "Categorize the block" }
                    p { class: "wl-modal-sub",
                        "The directive can't simply be swiped away. Tell the system what happened — it recalibrates, it never scolds."
                    }
                    input {
                        class: "wl-input",
                        r#type: "text",
                        placeholder: "Optional note (local only, encrypted at sync)",
                        value: "{note.read().clone()}",
                        oninput: move |e| note.set(e.value()),
                    }
                    button { class: "wl-bailout-reason", onclick: move |_| bail("external_dependency"),
                        div { class: "wl-bailout-reason-title", "External Dependency Blocked" }
                        div { class: "wl-bailout-reason-sub", "Waiting on a person, service, or asset outside your control." }
                    }
                    button { class: "wl-bailout-reason", onclick: move |_| bail("miscalculated_scope"),
                        div { class: "wl-bailout-reason-title", "Miscalculated Scope" }
                        div { class: "wl-bailout-reason-sub", "The task was significantly larger than planned. Estimate will be downsized and requeued." }
                    }
                    button { class: "wl-bailout-reason", onclick: move |_| bail("energy_depletion"),
                        div { class: "wl-bailout-reason-title", "Energy Depletion" }
                        div { class: "wl-bailout-reason-sub", "Cognitive or physical exhaustion. A low-cognitive recovery task is offered." }
                    }
                } else {
                    h2 { class: "wl-modal-header", "Keep the directive active?" }
                    p { class: "wl-modal-sub", "Closing the hatch without a category keeps your current directive." }
                    button {
                        class: "wl-btn-ghost",
                        onclick: move |_| { { let mut s = ctx.escape_open; *s.write() = false; } confirming.set(false); },
                        "Stay on directive"
                    }
                }
            }
        }
    }
}

async fn complete_current(ctx: &AppCtx) {
    let mut d = ctx.directive;
    match invoke::<DirectiveView>("complete_directive", ()).await {
        Ok(next) => {
            *d.write() = Some(next);
        }
        Err(e) => flash(ctx, &format!("ERR {e}")),
    }
}
