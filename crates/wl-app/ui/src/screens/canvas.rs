//! The Stackelberg Single-Directive Canvas (skill §4.A/B, US-3).
//!
//! Renders exactly ONE directive, and — since 2026-09-27 — exactly ONE
//! control: the hamburger. The footer that held "Complete Directive" and
//! "Bailout / Blocked" had already gone; what went with it this time were
//! the two keyboard gestures that reached the same state changes,
//! `⌘+Enter` and `Escape`, and the whole escape-hatch path behind the
//! latter.
//!
//! **The canvas is read-only.** A task is resolved from the ledger, and
//! that is a deliberate narrowing rather than a gap left by a deletion: the
//! escape hatch required a categorisation sheet to leave, and the sheet was
//! a modal on the one surface whose premise is that you are not
//! administrating. The recorded cost is real and is written down in PRD
//! delta 226 — nothing on this surface marks a task done, and the ledger
//! is one hamburger away. See also the "no session timer" note in
//! `app.rs`, which is the other thing this screen deliberately does not
//! have.

use dioxus::prelude::*;

use crate::app::{flash, invoke, set_directive, AppCtx, DirectiveView, VelocityView};

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
            // screen that floats over is what makes it a property of the
            // canvas rather than of the frame.
            class: "wl-root wl-canvas-screen",
            tabindex: "0",
            autofocus: "true",
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
        // an always-visible Evening audit button -- so nothing became
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
            // chevron uses -- one definition for one control language, so
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

        // Directive card (skill §4.B) -- the ONLY directive.
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
                        "Open the menu to create a goal, review settings, or mark a task done."
                    }
                }
            }
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

/// Refreshes the canvas's task from the shell.
///
/// Called after a tick on the ledger, which is the only place a task is
/// resolved — so the canvas has to be told rather than re-read on its own.
/// `pub` because the ledger screen is a different component and the
/// alternative is a second copy of this three-line sequence.
pub fn refresh_canvas(ctx: &AppCtx) {
    let ctx = *ctx;
    dioxus::core::spawn_forever(async move {
        match invoke::<Option<DirectiveView>>("current_directive", ()).await {
            Ok(dv) => set_directive(&ctx, dv),
            Err(e) => flash(&ctx, &format!("ERR {e}")),
        }
        // Velocity moves the moment a task resolves, not at the next
        // evening check-in — the HUD numbers read from the same signal.
        if let Ok(v) = invoke::<VelocityView>("velocity", ()).await {
            {
                let mut s = ctx.velocity;
                *s.write() = v;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The screen's own source, with the test module cut off.
    ///
    /// A source scan that includes the assertions can never pass: the
    /// assertion names the very string it is looking for. Cutting at the
    /// `cfg(test)` marker is what makes the guard check the MARKUP rather
    /// than the test that checks the markup.
    fn markup() -> &'static str {
        let source = include_str!("canvas.rs");
        source
            .split("#[cfg(test)]")
            .next()
            .expect("the source contains its own test module")
    }

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

    /// The canvas has no keydown handler any more, and that is the point
    /// rather than an accident of the refactor: `⌘+Enter` completed a task
    /// and `Escape` opened the categorisation sheet, and the user asked for
    /// a surface that cannot resolve anything. The guard below is the only
    /// way to hold that line from the other side — the markup simply has no
    /// `onkeydown` on it, so there is nothing for a keypress to reach.
    #[test]
    fn the_canvas_takes_no_input_but_its_menu() {
        let source = markup();
        assert!(
            !source.contains("onkeydown"),
            "the canvas must not bind any key handler; completion and the escape hatch moved to the ledger"
        );
        assert!(
            !source.contains("complete_directive"),
            "the canvas must not reach the shell's completion path"
        );
        assert!(
            !source.contains("bail_out"),
            "the escape hatch is gone; nothing may call it"
        );
    }
}
