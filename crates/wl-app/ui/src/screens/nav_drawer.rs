//! MVP-1 nav drawer: left slide-out with a dimmed full-screen overlay
//! behind it (the drawer itself is NOT full-screen). App name on top,
//! two HORIZONTAL icon buttons pinned to the bottom: goal creation
//! (LEFT, creation icon, contrasting highlight) → GoalCreate; settings
//! (RIGHT, gear icon) → Settings. Closes on Esc / backdrop click / tap.
//!
//! Separate from the Ctrl+, telemetry drawer: this is discovery
//! navigation (open app → hamburger → create goal → active directive).

use dioxus::prelude::*;

use crate::app::{AppCtx, Screen};

#[component]
pub fn NavDrawer() -> Element {
    let ctx = use_context::<AppCtx>();
    let close = move |_| {
        let mut n = ctx.nav_open;
        *n.write() = false;
    };
    let go = move |screen: Screen| {
        let ctx = ctx;
        move |_| {
            {
                let mut n = ctx.nav_open;
                *n.write() = false;
            }
            {
                let mut s = ctx.screen;
                *s.write() = screen.clone();
            }
        }
    };

    rsx! {
        div {
            class: "wl-modal-backdrop wl-nav-backdrop",
            onclick: close,
            role: "presentation",
            div {
                class: "wl-nav-sheet",
                role: "dialog",
                aria_label: "Navigation",
                onclick: move |e| e.stop_propagation(),
                h2 { class: "wl-nav-appname", "Worldline" }
                div { class: "wl-nav-spacer" }
                div { class: "wl-nav-actions",
                    button {
                        class: "wl-nav-btn wl-nav-btn-create",
                        aria_label: "Create a goal",
                        title: "Create a goal",
                        onclick: go(Screen::GoalCreate),
                        "✚"
                    }
                    button {
                        class: "wl-nav-btn wl-nav-btn-settings",
                        aria_label: "Settings",
                        title: "Settings",
                        onclick: go(Screen::Settings),
                        "⚙"
                    }
                }
            }
        }
    }
}
