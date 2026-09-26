//! Inline SVG icon set.
//!
//! These replace font glyphs (`☰`, `⚙`, `←`). That was not a cosmetic
//! swap — a glyph's weight, spacing and (on many systems) its colour are
//! chosen by the font stack, not by us. The drawer's settings icon in
//! particular rendered as a colour emoji on common Linux desktops, which
//! is where its "wrong colour, wrong size" drift came from. Every icon
//! here strokes with `currentColor`, so it inherits the active theme
//! tokens and the button's own `color` for free.
//!
//! Constraints that shaped this file:
//! * No icon font and no JS toolchain exist in this repo, and no build
//!   step copies assets beyond `public/`. Inline SVG needs none of them.
//! * Only `path` elements are used, and every geometry attribute is a
//!   string. Dioxus 0.7 types `viewBox`/`d`/`stroke-linecap` as strings
//!   and the numeric SVG attributes (`cx`, `r`, `x1`, …) as `f32`, so a
//!   paths-only set sidesteps the numeric ones entirely.
//! * Every icon is `aria-hidden`. Each one sits inside a control that
//!   already carries the real `aria-label`, so the icon must not be
//!   announced twice. (Dioxus 0.7 has no `focusable` attribute, but an
//!   SVG inside a button is not independently focusable regardless.)
//! * Sizing is CSS (`.wl-icon` + a per-icon modifier), never a
//!   `width`/`height` attribute, so one rule controls all of them.

use dioxus::prelude::*;

/// Shared presentation for every icon. Sizing is left to CSS.
fn frame(class: &'static str, view_box: &'static str, children: Element) -> Element {
    rsx! {
        svg {
            class: "{class} wl-icon",
            view_box: "{view_box}",
            fill: "none",
            stroke: "currentColor",
            stroke_width: "1.6",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            {children}
        }
    }
}

/// Navigation menu — the canvas's floating top-left control.
///
/// Three geometrically even rules (y = 5, 9, 13) spanning x = 3…15 in an
/// 18-unit box, with round caps. Deliberately still a hamburger: a menu
/// mark that does not read as a menu is a usability regression, and the
/// real defect was never the shape — it was that `☰` was a font glyph
/// whose stroke weight and spacing we could not set. This one we can.
#[component]
pub fn IconMenu() -> Element {
    frame(
        "wl-icon-menu",
        "0 0 18 18",
        rsx! {
            path { d: "M3 5h12M3 9h12M3 13h12" }
        },
    )
}

/// Settings — horizontal sliders.
///
/// Replaces `⚙`, the glyph most likely to render as a colour emoji. Three
/// rules with ring-shaped knobs parked at different positions reads as
/// "adjustable" at 20px, where a gear's teeth turn to mush, and it sits
/// closer to the terminal-tactile language than a gear does.
#[component]
pub fn IconSettings() -> Element {
    frame(
        "wl-icon-settings",
        "0 0 20 20",
        rsx! {
            path { d: "M2.5 5h15M2.5 10h15M2.5 15h15" }
            // Knob rings, r = 2.1, each drawn as two half-arcs so the
            // path closes on itself.
            path { d: "M4.9 5a2.1 2.1 0 1 0 4.2 0a2.1 2.1 0 1 0-4.2 0Z" }
            path { d: "M10.9 10a2.1 2.1 0 1 0 4.2 0a2.1 2.1 0 1 0-4.2 0Z" }
            path { d: "M6.9 15a2.1 2.1 0 1 0 4.2 0a2.1 2.1 0 1 0-4.2 0Z" }
        },
    )
}

/// Back — a chevron, floated over the page header's left gutter.
///
/// Same control as before, same hit target, same `aria-label`; only the
/// drawing changed, so it now matches the two icons beside it instead of
/// being the one glyph on the page.
#[component]
pub fn IconBack() -> Element {
    frame(
        "wl-icon-back",
        "0 0 18 18",
        rsx! {
            path { d: "M11 3.5 5.5 9l5.5 5.5" }
        },
    )
}

/// Moon — drawn in the theme switch knob when the knob is in the "light
/// theme selected" position. Filled rather than stroked: at 11px a
/// crescent's thin inner limb disappears, but a solid one still reads.
#[component]
pub fn IconMoon() -> Element {
    rsx! {
        svg {
            class: "wl-icon-moon wl-icon",
            view_box: "0 0 16 16",
            fill: "currentColor",
            stroke: "none",
            "aria-hidden": "true",
            path { d: "M13.5 10.5A6 6 0 0 1 6.5 3.5a6.2 6.2 0 1 0 7 7Z" }
        }
    }
}

/// Sun — the theme switch knob's other state: a ring plus eight rays
/// spanning r = 5.0…6.6, clear of the r = 3.2 ring.
#[component]
pub fn IconSun() -> Element {
    frame(
        "wl-icon-sun",
        "0 0 16 16",
        rsx! {
            path { d: "M8 4.8a3.2 3.2 0 1 0 0 6.4 3.2 3.2 0 1 0 0-6.4Z" }
            path { d: "M8 1.4v1.6M8 13v1.6M1.4 8h1.6M13 8h1.6M3.35 3.35l1.13 1.13M11.52 11.52l1.13 1.13M12.65 3.35l-1.13 1.13M4.48 11.52l-1.13 1.13" }
        },
    )
}
