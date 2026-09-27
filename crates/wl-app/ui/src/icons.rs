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

/// Settings — a gear.
///
/// Replaces `⚙` (the glyph most likely to render as a colour emoji) and
/// then a sliders mark that read as "audio mixer" rather than
/// "preferences".
///
/// One filled path with `fill-rule="evenodd"`: the outer sub-path is the
/// notched rim, and the trailing circle sub-path is the hub hole, which
/// evenodd knocks out. That is why this icon is `fill: currentColor`
/// rather than the stroked `frame()` the other icons use — a gear drawn
/// with strokes reads as a sun or a ship's wheel, both of which were
/// tried and rendered before settling on the filled outline.
///
/// Proportions were measured off the reference mark rather than guessed:
/// outer r = 8.6 on a 24-unit grid, hub hole r = 3.4 (≈0.40 of the outer
/// radius, matching the reference), root r = 7.0 for the notches, and 8
/// teeth of ±15°. Shallower notches and a wider notch gap both turned
/// the mark into a ring or a skeleton, so those are the tuned values.
#[component]
pub fn IconSettings() -> Element {
    rsx! {
        svg {
            class: "wl-icon-settings wl-icon",
            view_box: "0 0 24 24",
            fill: "currentColor",
            fill_rule: "evenodd",
            stroke: "none",
            "aria-hidden": "true",
            path { d: "M20.31 9.77A8.6 8.6 0 0 1 20.31 14.23L18.76 13.81A7 7 0 0 1 18.06 15.5L19.45 16.3M19.45 16.3A8.6 8.6 0 0 1 16.3 19.45L15.5 18.06A7 7 0 0 1 13.81 18.76L14.23 20.31M14.23 20.31A8.6 8.6 0 0 1 9.77 20.31L10.19 18.76A7 7 0 0 1 8.5 18.06L7.7 19.45M7.7 19.45A8.6 8.6 0 0 1 4.55 16.3L5.94 15.5A7 7 0 0 1 5.24 13.81L3.69 14.23M3.69 14.23A8.6 8.6 0 0 1 3.69 9.77L5.24 10.19A7 7 0 0 1 5.94 8.5L4.55 7.7M4.55 7.7A8.6 8.6 0 0 1 7.7 4.55L8.5 5.94A7 7 0 0 1 10.19 5.24L9.77 3.69M9.77 3.69A8.6 8.6 0 0 1 14.23 3.69L13.81 5.24A7 7 0 0 1 15.5 5.94L16.3 4.55M16.3 4.55A8.6 8.6 0 0 1 19.45 7.7L18.06 8.5A7 7 0 0 1 18.76 10.19L20.31 9.77ZM15.4 12A3.4 3.4 0 1 0 8.6 12A3.4 3.4 0 1 0 15.4 12Z" }
        }
    }
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
