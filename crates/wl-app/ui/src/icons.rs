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
/// theme selected" position.
///
/// The geometry is Feather's `moon` (ISC-licensed, the same provenance as
/// the other Feather-derived metrics in this file's lineage) on a 24-unit
/// grid rather than a hand-rolled 16-unit crescent. The hand-rolled one
/// was *correct* but at a 12px render box its inner limb and its outer
/// arc collapsed into an unreadable blob — the terminator needs more
/// grid than 16 units leaves it once it is scaled down.
#[component]
pub fn IconMoon() -> Element {
    rsx! {
        svg {
            class: "wl-icon-moon wl-icon",
            view_box: "0 0 24 24",
            fill: "currentColor",
            stroke: "none",
            "aria-hidden": "true",
            path { d: "M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79Z" }
        }
    }
}

/// Sun — the theme switch knob's other state. A stroked ring plus eight
/// rays, on the same 24-unit grid as the moon so the two are optically
/// the same weight at the same box.
#[component]
pub fn IconSun() -> Element {
    frame(
        "wl-icon-sun",
        "0 0 24 24",
        rsx! {
            // Ring, r = 5, drawn as two half-arcs.
            path { d: "M12 7a5 5 0 1 0 0 10 5 5 0 1 0 0-10Z" }
            path { d: "M12 1v2M12 21v2M4.22 4.22l1.42 1.42M18.36 18.36l1.42 1.42M1 12h2M21 12h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42" }
        },
    )
}
