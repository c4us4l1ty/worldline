//! The Stackelberg Single-Directive Canvas (skill §4.A/B/C, US-3).
//!
//! Renders exactly ONE directive. ⌘+Enter completes; Escape opens the
//! frictionful escape hatch modal requiring categorization.

use dioxus::prelude::*;

use crate::app::{elapsed_secs, flash, fmt_mmss, invoke, set_directive, AppCtx, DirectiveView};

pub fn CanvasScreen() -> Element {
    let ctx = use_context::<AppCtx>();

    // Load the current directive on mount.
    use_effect(move || {
        let mut busy = ctx.directive_busy;
        if *busy.peek() {
            return;
        }
        busy.set(true);
        dioxus::core::spawn_forever(async move {
            match invoke::<Option<DirectiveView>>("current_directive", ()).await {
                Ok(dv) => set_directive(&ctx, dv),
                Err(e) => {
                    set_directive(&ctx, None);
                    flash(&ctx, &format!("ERR {e}"));
                }
            }
            busy.set(false);
        });
    });

    let d = ctx.directive.read().clone();
    let unavailable = d.is_none() || *ctx.directive_busy.read();
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
                if e.is_auto_repeating() {
                    return;
                }
                // Telemetry drawer takes precedence on Escape.
                if *ctx.telemetry_open.read() {
                    return;
                }
                // ⌘+Enter / Ctrl+Enter completes; Escape toggles bailout (skill §7.5).
                if e.key() == Key::Enter && (e.modifiers().meta() || e.modifiers().ctrl()) {
                    if !*ctx.escape_open.read() {
                        complete_current(&ctx);
                    }
                } else if e.key() == Key::Escape
                    && !*ctx.directive_busy.peek() && ctx.directive.peek().is_some() {
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
                    title: "System telemetry (Ctrl+,)",
                    style: "border: 1px solid var(--wl-border-subtle); cursor: pointer; font-family: var(--font-mono);",
                    onclick: move |_| { { let mut s = ctx.telemetry_open; *s.write() = true; } },
                    if let Some(session) = ctx.timer_session.read().clone() {
                        TimerDisplay { key: "{session.started_ms}", started_ms: session.started_ms }
                    } else {
                        "00:00"
                    }
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
                disabled: unavailable || escaping,
                onclick: move |_| complete_current(&ctx),
                span { "Complete Directive" }
                kbd { class: "wl-kbd", "⌘↵" }
            }
            button { class: "wl-btn-escape",
                disabled: unavailable,
                onclick: move |_| {
                    if !*ctx.directive_busy.peek() && ctx.directive.peek().is_some() {
                        let mut s = ctx.escape_open;
                        s.set(true);
                    }
                },
                span { "Bailout / Blocked" }
                kbd { class: "wl-kbd-subtle", "Esc" }
            }
        }

        // Frictionful escape hatch modal (skill §4.C, §5 "Do")
        if escaping {
            EscapeModal {}
        }
        }
    }
}

#[wasm_bindgen::prelude::wasm_bindgen(inline_js = r#"
export function start_timer(tick) {
    let timeout;
    let stopped = false;
    function update() {
        clearTimeout(timeout);
        if (stopped || document.hidden) return;
        tick();
        timeout = setTimeout(update, 1000);
    }
    document.addEventListener('visibilitychange', update);
    if (!document.hidden) timeout = setTimeout(update, 1000);
    return () => {
        stopped = true;
        clearTimeout(timeout);
        document.removeEventListener('visibilitychange', update);
    };
}
"#)]
extern "C" {
    fn start_timer(tick: &js_sys::Function) -> js_sys::Function;
}

struct TimerSubscription {
    stop: js_sys::Function,
    _tick: wasm_bindgen::closure::Closure<dyn FnMut()>,
}

impl Drop for TimerSubscription {
    fn drop(&mut self) {
        let _ = self.stop.call0(&wasm_bindgen::JsValue::NULL);
    }
}

#[component]
fn TimerDisplay(started_ms: f64) -> Element {
    use wasm_bindgen::JsCast;

    let mut seconds = use_signal(|| elapsed_secs(started_ms, js_sys::Date::now()));
    use_hook(move || {
        let tick = wasm_bindgen::closure::Closure::wrap(Box::new(move || {
            seconds.set(elapsed_secs(started_ms, js_sys::Date::now()));
        }) as Box<dyn FnMut()>);
        std::rc::Rc::new(TimerSubscription {
            stop: start_timer(tick.as_ref().unchecked_ref()),
            _tick: tick,
        })
    });
    let timer = fmt_mmss(*seconds.read());
    rsx! { "{timer}" }
}

#[component]
pub fn DirectiveCard(d: DirectiveView) -> Element {
    let phase = valid_phase(d.phase);
    let phase_badge = match phase {
        Some((step, total)) => format!(
            "Phase {step} of {total} · {} min",
            d.estimated_minutes.max(0)
        ),
        None => format!("{} Minutes", d.estimated_minutes.max(0)),
    };
    let progress = match phase {
        Some((step, total)) => (step as f64 / total as f64) * 100.0,
        _ => 100.0,
    };
    rsx! {
        div { class: "wl-directive-card wl-card-enter",
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

fn valid_phase(phase: Option<(i64, i64)>) -> Option<(i64, i64)> {
    phase.filter(|&(step, total)| step >= 1 && step <= total)
}

#[derive(Clone, Copy, PartialEq)]
enum BailReason {
    Dependency,
    Scope,
    Energy,
}

impl BailReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Dependency => "external_dependency",
            Self::Scope => "miscalculated_scope",
            Self::Energy => "energy_depletion",
        }
    }

    fn from_key(key: &Key) -> Option<Self> {
        match key {
            Key::Character(c) if c == "1" => Some(Self::Dependency),
            Key::Character(c) if c == "2" => Some(Self::Scope),
            Key::Character(c) if c == "3" => Some(Self::Energy),
            _ => None,
        }
    }
}

#[component]
fn EscapeModal() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut note = use_signal(String::new);
    let mut selected = use_signal(|| None::<BailReason>);
    let mut confirming = use_signal(|| false);

    let bail = move || {
        let Some(reason) = *selected.peek() else {
            return;
        };
        if !*ctx.escape_open.peek() || *ctx.telemetry_open.peek() || !begin_directive_action(&ctx) {
            return;
        }
        let note = note.read().clone();
        let reason = reason.as_str().to_string();
        dioxus::core::spawn_forever(async move {
            #[derive(serde::Serialize)]
            struct BailReq {
                reason: String,
                note: Option<String>,
            }
            // Spec cap: 140 chars of optional context.
            let trimmed: String = note.chars().take(140).collect();
            let req = BailReq {
                reason,
                note: if trimmed.trim().is_empty() {
                    None
                } else {
                    Some(trimmed)
                },
            };
            match invoke::<serde_json::Value>("bail_out", req).await {
                Ok(_) => {
                    let mut escape = ctx.escape_open;
                    *escape.write() = false;
                    flash(&ctx, "VELOCITY ADJUSTED — NO GUILT");
                    match invoke::<Option<DirectiveView>>("current_directive", ()).await {
                        Ok(dv) => set_directive(&ctx, dv),
                        Err(e) => {
                            set_directive(&ctx, None);
                            flash(&ctx, &format!("ERR {e}"));
                        }
                    }
                }
                Err(e) => flash(&ctx, &format!("ERR {e}")),
            }
            let mut busy = ctx.directive_busy;
            busy.set(false);
        });
    };

    // Keyboard: 1/2/3 categorize, Esc resumes (spec §1 bindings).
    let note_len = note.read().chars().count();
    let busy = *ctx.directive_busy.read();

    rsx! {
        div {
            class: "wl-modal-backdrop wl-modal-centered",
            tabindex: "0",
            autofocus: "true",
            onkeydown: move |e: Event<KeyboardData>| {
                if *ctx.telemetry_open.peek()
                    || (e.key() == Key::Character(",".to_string()) && e.modifiers().ctrl()) {
                    return;
                }
                e.stop_propagation();
                if e.is_auto_repeating() || *ctx.directive_busy.peek() {
                    return;
                }
                if e.key() == Key::Escape {
                    let mut s = ctx.escape_open;
                    s.set(false);
                } else if !*confirming.peek() && e.modifiers().is_empty() {
                    if let Some(reason) = BailReason::from_key(&e.key()) {
                        selected.set(Some(reason));
                    }
                }
            },
            onclick: move |_| {
                if !*ctx.directive_busy.peek() {
                    confirming.set(true);
                }
            },
            div { class: "wl-modal-sheet wl-modal-centered-sheet", onclick: move |e| e.stop_propagation(),
                if !*confirming.read() {
                    h2 { class: "wl-modal-header", "Diagnostic: escape hatch" }
                    p { class: "wl-modal-sub",
                        "Select stall cause. Worldline will recalibrate your velocity with zero punitive alarms."
                    }
                    input {
                        class: "wl-input",
                        r#type: "text",
                        maxlength: "140",
                        placeholder: "Optional context (max 140 chars)",
                        value: "{note.read().clone()}",
                        disabled: busy,
                        onkeydown: move |e: Event<KeyboardData>| {
                            if BailReason::from_key(&e.key()).is_some() {
                                e.stop_propagation();
                            }
                        },
                        oninput: move |e| {
                            let v: String = e.value().chars().take(140).collect();
                            note.set(v);
                        },
                    }
                    p { class: "wl-char-count wl-mono", "{note_len}/140" }
                    button { class: "wl-bailout-reason", disabled: busy,
                        aria_pressed: selected.read().as_ref() == Some(&BailReason::Dependency),
                        onclick: move |_| { if !*ctx.directive_busy.peek() { selected.set(Some(BailReason::Dependency)); } },
                        div { class: "wl-bailout-reason-title", "[1] Dependency blocked" }
                        div { class: "wl-bailout-reason-sub", "Waiting for 3rd party API, merge, or response." }
                    }
                    button { class: "wl-bailout-reason", disabled: busy,
                        aria_pressed: selected.read().as_ref() == Some(&BailReason::Scope),
                        onclick: move |_| { if !*ctx.directive_busy.peek() { selected.set(Some(BailReason::Scope)); } },
                        div { class: "wl-bailout-reason-title", "[2] Scope miscalculation" }
                        div { class: "wl-bailout-reason-sub", "Directive exceeds allotted time boundary (>2x). Dispatcher splits it; quota untouched." }
                    }
                    button { class: "wl-bailout-reason", disabled: busy,
                        aria_pressed: selected.read().as_ref() == Some(&BailReason::Energy),
                        onclick: move |_| { if !*ctx.directive_busy.peek() { selected.set(Some(BailReason::Energy)); } },
                        div { class: "wl-bailout-reason-title", "[3] Cognitive / energy depletion" }
                        div { class: "wl-bailout-reason-sub", "Focus ceiling reached; request a downscaled task + 10-minute rest." }
                    }
                    p { class: "wl-modal-sub",
                        match *selected.read() {
                            Some(BailReason::Dependency) => "Selected: dependency blocked",
                            Some(BailReason::Scope) => "Selected: scope miscalculation",
                            Some(BailReason::Energy) => "Selected: cognitive / energy depletion",
                            None => "Select a category before confirming.",
                        }
                    }
                    button { class: "wl-btn-primary", disabled: busy || selected.read().is_none(), onclick: move |_| bail(),
                        "Confirm bailout"
                    }
                    button {
                        class: "wl-btn-escape",
                        disabled: busy,
                        onclick: move |_| {
                            if !*ctx.directive_busy.peek() {
                                let mut s = ctx.escape_open;
                                s.set(false);
                            }
                        },
                        span { "Resume execution directive" }
                        kbd { class: "wl-kbd-subtle", "Esc" }
                    }
                } else {
                    h2 { class: "wl-modal-header", "Keep the directive active?" }
                    p { class: "wl-modal-sub", "Closing the hatch without a category keeps your current directive." }
                    button {
                        class: "wl-btn-ghost",
                        disabled: busy,
                        onclick: move |_| {
                            if !*ctx.directive_busy.peek() {
                                let mut s = ctx.escape_open;
                                s.set(false);
                                confirming.set(false);
                            }
                        },
                        "Stay on directive"
                    }
                }
            }
        }
    }
}

fn claim_action(busy: &mut bool, has_directive: bool) -> bool {
    if *busy || !has_directive {
        return false;
    }
    *busy = true;
    true
}

fn begin_directive_action(ctx: &AppCtx) -> bool {
    let mut busy = ctx.directive_busy;
    let claimed = claim_action(&mut busy.write(), ctx.directive.peek().is_some());
    claimed
}

fn complete_current(ctx: &AppCtx) {
    if *ctx.escape_open.peek() || !begin_directive_action(ctx) {
        return;
    }
    let ctx = *ctx;
    dioxus::core::spawn_forever(async move {
        match invoke::<Option<DirectiveView>>("complete_directive", ()).await {
            Ok(next) => set_directive(&ctx, next),
            Err(e) => flash(&ctx, &format!("ERR {e}")),
        }
        let mut busy = ctx.directive_busy;
        busy.set(false);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_phase_metadata_is_not_rendered() {
        for phase in [
            None,
            Some((0, 2)),
            Some((-1, 2)),
            Some((1, 0)),
            Some((3, 2)),
        ] {
            assert_eq!(valid_phase(phase), None);
        }
        assert_eq!(valid_phase(Some((1, 2))), Some((1, 2)));
        assert_eq!(
            valid_phase(Some((i64::MAX, i64::MAX))),
            Some((i64::MAX, i64::MAX))
        );
    }

    #[test]
    fn category_shortcuts_require_an_explicit_supported_digit() {
        for (key, reason) in [
            ("1", "external_dependency"),
            ("2", "miscalculated_scope"),
            ("3", "energy_depletion"),
        ] {
            assert_eq!(
                BailReason::from_key(&Key::Character(key.into())).map(BailReason::as_str),
                Some(reason)
            );
        }
        for key in [Key::Enter, Key::Escape, Key::Character("4".into())] {
            assert!(BailReason::from_key(&key).is_none());
        }
    }

    #[test]
    fn shared_action_guard_rejects_reentry_until_released() {
        let mut busy = false;
        assert!(!claim_action(&mut busy, false));
        assert!(!busy);
        assert!(claim_action(&mut busy, true));
        assert!(!claim_action(&mut busy, true));
        assert!(busy);
        busy = false;
        assert!(claim_action(&mut busy, true));
    }
}
