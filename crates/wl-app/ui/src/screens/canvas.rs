//! The Stackelberg Single-Directive Canvas (skill §4.A/B, US-3).
//!
//! Renders exactly ONE directive and, since 2026-09-27, exactly ONE
//! control: the hamburger. The footer that held "Complete Directive" and
//! "Bailout / Blocked" is gone — the canvas is the directive, not a
//! dashboard — so completion is `⌘+Enter` and the escape hatch is
//! `Escape`, both still bound above. The escape modal itself is
//! untouched: it is still the frictionful categorisation, just reached by
//! key rather than by a second button competing with the card.

use dioxus::prelude::*;

use crate::app::{
    flash, invoke, is_telemetry_chord, set_directive, AppCtx, DirectiveView, VelocityView,
};

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
    let escaping = *ctx.escape_open.read();

    rsx! {
        div {
            // `wl-canvas-screen` and not a bare `wl-root`: `.wl-float-layer` is
            // `position: absolute`, and with no positioned ancestor on
            // this subtree its containing block was `#main` -- the APP
            // FRAME, not the screen. Two consequences, both invisible in
            // the source: the control's `left: 0` was measured from the
            // frame's padding box, so the hamburger sat 2px OUTSIDE the
            // content box on which the directive card is laid out and
            // 8px above its top edge; and the control's position was
            // decided by an ancestor that also hosts the drawer, the
            // toast and the error screen, so anything that ever gave
            // `#main` a transform, a filter or a `contain` would move the
            // one control the canvas has. Anchoring the layer to the
            // screen it floats over is what makes it a property of the
            // canvas rather than of the frame.
            class: "wl-root wl-canvas-screen",
            tabindex: "0",
            autofocus: "true",
            onkeydown: move |e: Event<KeyboardData>| {
                if e.is_auto_repeating() {
                    return;
                }
                // A panel that is already up takes precedence on Escape,
                // and the panel is what Escape belongs to.
                //
                // Only the TELEMETRY drawer used to be checked. With the
                // control panel open, Escape therefore did two things at
                // once: this handler opened the bailout sheet, and the
                // app-root handler (which runs afterwards, on the same
                // bubbled keydown) closed the panel. The user pressed one
                // key to dismiss a menu and got a modal instead, on top
                // of a canvas that had not changed -- which reads, from
                // the outside, exactly like "the menu does not open".
                if *ctx.telemetry_open.read() || *ctx.nav_open.read() {
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
        // The canvas's only chrome.
        //
        // The header used to be a solid strip spanning the full width with
        // a `border-bottom`, carrying six elements: the menu, an Evening
        // audit nudge, the milestone label, sync status, a sync-now button
        // and the timer. That bar dominated a canvas whose entire premise is
        // ONE directive and no chrome, and it squeezed the empty state into
        // a letterbox. The menu is now a floating circular control
        // overlaying the content the way the reference design does.
        //
        // Milestone, sync status, sync-now and Evening audit moved to the
        // telemetry drawer (Ctrl+,), which already rendered sync stats and
        // an always-visible Evening audit button — so nothing became
        // unreachable. The session timer was removed entirely (2026-09-26):
        // a focus timer you cannot see is not a timer, and elapsed time
        // survives only as sync freshness in the telemetry drawer.
        div { class: "wl-float-layer",
            // The layer spans the full width but must stay transparent to
            // input, so `pointer-events` is re-enabled on the control.
            //
            // The sync control that used to float opposite this was a 34px
            // circle that reported nothing until tapped; sync is a settings
            // concern, not a canvas one, so it moved to the Sync section of
            // Settings, where it is a full-width row that also shows what
            // the last cycle did.
            //
            // `.wl-circle-btn` is the same 38px circle every page's back
            // chevron uses — one definition for one control language, so
            // this and the button at the top of Settings cannot drift apart.
            button {
                class: "wl-circle-btn",
                aria_label: "Open navigation",
                title: "Navigation",
                // `open_nav`, not a raw write: the panel's open/closed
                // flag has one owner (`AppCtx::open_nav` / `close_nav`),
                // and this is its only opener. `wl-float-menu` is gone
                // with its dead rule -- the modifier set
                // `position: static`, which is what a non-positioned
                // button already is, so the class bought nothing and its
                // "specificity" was a mirage.
                onclick: move |_| ctx.open_nav(),
                crate::icons::IconMenu {}
            }
        }

        // Directive card (skill §4.B) — the ONLY directive.
        main { class: "wl-directive-container",
            if let Some(d) = d {
                DirectiveCard { d: d }
            } else {
                // No CTA here. The empty state used to carry a ghost
                // "Open menu" button under this copy, which duplicated
                // the floating hamburger sitting 14px above it and made
                // the canvas's one piece of chrome look like two. The
                // line below names the affordance instead, and the
                // hamburger is always on screen to press.
                div { class: "wl-directive-card",
                    h1 { class: "wl-serif-title", "No active directive." }
                    p { class: "wl-body-muted",
                        "Open the menu to create a goal or review settings."
                    }
                }
            }
        }


        // Frictionful escape hatch modal (skill §4.C, §5 "Do")
        if escaping {
            EscapeModal {}
        }
        }
    }
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
                if *ctx.telemetry_open.peek() {
                    return;
                }
                if is_telemetry_chord(&e) {
                    // The chord belongs to the app root, so this event is
                    // let through deliberately rather than swallowed.
                    // The bailout modal is closed first, though: both
                    // overlays are `z-index: 40` siblings and the drawer
                    // comes later in the DOM, so leaving the modal open
                    // stacked a sheet on top of a sheet the user could no
                    // longer reach. Escape only clears the drawers, so the
                    // modal reappeared underneath and had to be dismissed
                    // a second time.
                    let mut s = ctx.escape_open;
                    s.set(false);
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
            Ok(next) => {
                set_directive(&ctx, next);
                // MVP-3: velocity must move the moment a directive
                // completes, not wait for the next evening check-in
                // (the HUD target/ratio live in the same signal the
                // telemetry drawer and check-in read).
                if let Ok(v) = invoke::<VelocityView>("velocity", ()).await {
                    {
                        let mut s = ctx.velocity;
                        *s.write() = v;
                    }
                }
            }
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
