//! Evening Check-In — the 30-second objective audit (PRD §5.4).
//! Done / Partial / Skipped. Zero-guilt GPS recalibration display.

use dioxus::prelude::*;

use crate::app::{flash, invoke, AppCtx, Screen, VelocityView};

/// The one-shot claim rule for a check-in, as a pure function: at most
/// one write, ever, and never while another is in flight.
///
/// Deliberately pure. Both flags it reads are component-scoped and are
/// released from a task that outlives the component, so the *signal*
/// writes live at the call site (and go through `app::set_if_alive`,
/// because a dropped scope is a panic and a panic on wasm32 aborts the
/// module). The rule they implement is what is worth pinning here.
fn claim_submission(busy: &mut bool, submitted: bool) -> bool {
    if *busy || submitted {
        return false;
    }
    *busy = true;
    true
}

pub fn CheckInScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut note = use_signal(String::new);
    let submitted = use_signal(|| false);
    let mut busy = use_signal(|| false);

    let mut record = move |outcome: &str| {
        let mut in_flight = *busy.peek();
        if !claim_submission(&mut in_flight, *submitted.peek()) {
            return;
        }
        busy.set(true);
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
                    // `ctx.velocity` is root-scoped and always safe. The
                    // two component signals are not: this task runs at
                    // the root scope, so the check-in page can be gone
                    // (back pressed during the write) before the reply
                    // lands, and a direct `submitted.set(..)` on a
                    // dropped scope panics — which, on wasm32, aborts the
                    // module and freezes the window on its last frame
                    // with no error screen. See `app::set_if_alive`.
                    {
                        let mut s = ctx.velocity;
                        *s.write() = v;
                    }
                    crate::app::set_if_alive(&submitted, true);
                    flash(&ctx, "LOGGED — OBJECTIVELY");
                }
                Err(e) => flash(&ctx, &format!("ERR {e}")),
            }
            // Released on both arms, and only if the page is still here.
            crate::app::set_if_alive(&busy, false);
        });
    };

    let v = ctx.velocity.read().clone();
    let pct = (v.estimate_adjustment * 100.0).round();

    rsx! {
        div {
            class: "wl-scroll-region",
            tabindex: "0",
            h1 { class: "wl-serif-title",
                "Evening " span { class: "wl-italic-accent", "recalibration" }
            }
            p { class: "wl-body-muted", style: "margin-bottom: 18px;",
                "Today's vector audit. Skips are velocity adjustments, not failures — the system recalibrates like a GPS, never like an alarm."
            }

            if !*submitted.read() {
                div { class: "wl-fieldset",
                    textarea {
                        class: "wl-textarea",
                        placeholder: "Optional: what happened today?",
                        disabled: *busy.read(),
                        value: "{note.read().clone()}",
                        oninput: move |e| note.set(e.value()),
                    }
                }
                div { class: "wl-checkin-row",
                    button { class: "wl-checkin-choice", disabled: *busy.read(), onclick: move |_| record("done"),
                        span { class: "wl-checkin-glyph", "●" }
                        span { class: "wl-checkin-label", "Done" }
                    }
                    button { class: "wl-checkin-choice", disabled: *busy.read(), onclick: move |_| record("partial"),
                        span { class: "wl-checkin-glyph", "◐" }
                        span { class: "wl-checkin-label", "Partial" }
                    }
                    button { class: "wl-checkin-choice", disabled: *busy.read(), onclick: move |_| record("skipped"),
                        span { class: "wl-checkin-glyph", "○" }
                        span { class: "wl-checkin-label", "Skipped" }
                    }
                }
            } else {
                div { class: "wl-directive-card", style: "margin-bottom: 12px; padding: 18px;",
                    div { class: "wl-directive-step-badge", "Trajectory computation" }
                    p { class: "wl-body-muted wl-mono", "Remaining scope: {v.milestones_remaining} directives" }
                    p { class: "wl-body-muted wl-mono", "Days to horizon: {v.days_remaining} days" }
                    p { class: "wl-body-muted wl-mono", "Required velocity: {v.target_per_day:.2} / day" }
                    p { class: "wl-body-muted wl-mono", "Rolling average: {v.completion_ratio:.2} · observed adjustment {pct}% (not yet applied to future estimates)" }
                    p { class: "wl-body-muted", style: "margin-top: 8px;",
                        "V_target = remaining milestones / remaining days. No debt carried forward. Plan recalculated cleanly."
                    }
                }
                p { class: "wl-body-muted", style: "margin-top: 4px;",
                    "Trajectory recorded. Tomorrow's plans still use raw estimates — the observed adjustment above is not applied yet."
                }
                button {
                    class: "wl-btn-primary",
                    style: "margin-top: 14px;",
                    onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::Dormant; } },
                    "Commit vector & rest"
                }
                button {
                    class: "wl-btn-ghost",
                    style: "margin-top: 8px;",
                    onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::Canvas; } },
                    "Return to the line"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::claim_submission;

    #[test]
    fn submission_rejects_duplicates_and_allows_retry_after_failure() {
        let mut busy = false;
        assert!(claim_submission(&mut busy, false));
        assert!(!claim_submission(&mut busy, false));
        busy = false;
        assert!(claim_submission(&mut busy, false));
        busy = false;
        assert!(!claim_submission(&mut busy, true));
        assert!(!busy);
    }
}
