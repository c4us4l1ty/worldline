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
///
/// Rendered at 20px (was 18px) so the canvas's one floating control has
/// presence at a glance. The 18-unit grid and its 1.6 stroke are
/// unchanged, so the mark also gained weight for free — the stroke now
/// lands at ~1.8 physical pixels.
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
/// cog, and the trailing circle sub-path is the hub hole, which evenodd
/// knocks out. That is why this icon is `fill: currentColor` rather than
/// the stroked `frame()` the other icons use — a gear drawn with strokes
/// reads as a sun or a ship's wheel, both of which were tried and
/// rendered before settling on the filled outline.
///
/// **Six teeth, and the gaps have to be visible.** The previous mark was
/// 8 teeth spanning 44° of a 45° pitch: the notches were 1° slivers,
/// which is 0.1px at render size, so they vanished and the mark read as
/// a lumpy washer rather than a cog. At 22px, six teeth of ±13°/±22°
/// leave a 1.95px gap between teeth and 3.3px per tooth — enough for the
/// silhouette to survive the downscale, which is the whole test here.
///
/// Geometry on a 24-unit grid: tip r = 10.5 (the old mark used only
/// 8.6, so it drew a 14.3px-diameter mark inside a 20px box and lost the
/// optical comparison against the bold coral `✚` beside it), root
/// r = 7.6, hub r = 3.0. Real ink is now 19.25px across.
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
            path { d: "M19.05 9.15 L22.23 9.64 A10.5 10.5 0 0 1 22.23 14.36 L19.05 14.85 A7.6 7.6 0 0 1 17.99 16.68 M17.99 16.68 L19.16 19.68 A10.5 10.5 0 0 1 15.07 22.04 L13.06 19.53 A7.6 7.6 0 0 1 10.94 19.53 M10.94 19.53 L8.93 22.04 A10.5 10.5 0 0 1 4.84 19.68 L6.01 16.68 A7.6 7.6 0 0 1 4.95 14.85 M4.95 14.85 L1.77 14.36 A10.5 10.5 0 0 1 1.77 9.64 L4.95 9.15 A7.6 7.6 0 0 1 6.01 7.32 M6.01 7.32 L4.84 4.32 A10.5 10.5 0 0 1 8.93 1.96 L10.94 4.47 A7.6 7.6 0 0 1 13.06 4.47 M13.06 4.47 L15.07 1.96 A10.5 10.5 0 0 1 19.16 4.32 L17.99 7.32 A7.6 7.6 0 0 1 19.05 9.15 M9 12a3 3 0 1 0 6 0a3 3 0 1 0 -6 0Z" }
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

/// Entropy — a run that decayed.
///
/// The control-panel's Entropy Log: every directive that was bailed out of.
/// The mark is deliberately NOT a warning triangle. This product bans red
/// failure states outright (skill §30: skips are "velocity adjustments",
/// never alarms), so a hazard sign would contradict the one rule the whole
/// visual language is built on. It is also not a clock or a magnifier,
/// which both belong to other pages.
///
/// What it is: a trace that rises, breaks, and resumes lower. The gap is
/// the whole idea — the run did not fail loudly, it lost its line and
/// picked one up somewhere else. Drawn on the same 18-unit grid as the
/// menu so the two canvas-adjacent marks share a weight.
#[component]
pub fn IconEntropy() -> Element {
    frame(
        "wl-icon-entropy",
        "0 0 18 18",
        rsx! {
            path { d: "M2.5 12.5 6 9l2.5 2.2" }
            path { d: "M11.5 9.5 15.5 6" }
            path { d: "M6 9 8.5 11.2 10 9.5" }
        },
    )
}

/// Velocity — a gauge arc with a needle.
///
/// The control panel's Trajectory page: required velocity per day against
/// what is actually being observed. A gauge rather than a rising line
/// because `IconEntropy` already owns the "line that goes somewhere" idea,
/// and the two sit twelve pixels apart in the same drawer — they have to
/// be told apart at a glance, not parsed.
///
/// The needle is drawn at roughly 2 o'clock, where the needle points when
/// the user is ahead of the required rate. A gauge resting at its low
/// corner would read as failure, which is the same mistake the entropy
/// mark avoids.
#[component]
pub fn IconVelocity() -> Element {
    frame(
        "wl-icon-velocity",
        "0 0 18 18",
        rsx! {
            path { d: "M2.75 13.25a7.5 7.5 0 0 1 12.5 0" }
            path { d: "M9 13.25 12.4 7.9" }
            path { d: "M9 13.25h.01" }
        },
    )
}
