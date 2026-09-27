---
name: worldline
description: Worldline Design System — Stackelberg single-directive 9:16 viewport (420px × 747px), tactile matte graphite tokens (#131312 / #20201F / #E26D52), calm zero-guilt velocity UI. Enforces directive command hierarchy, a read-only canvas with the task ledger as the only writer, a staged plan preview with a dependency spine, and the BIP-39 seed vault.
---

## What I do
- Enforce the Calm, Sovereign, & Tactile visual language for the **Worldline** client across desktop and mobile.
- Enforce the **9:16 vertical terminal viewport constraint** (`420px × 747px` fixed on desktop; fluid-contained on mobile).
- Codify the Stackelberg interface hierarchy:
  1. **Directive Command** (`DM Sans` Bold 700, `-0.03em`): non-negotiable active execution directive.
  2. **Micro-HUD & Metadata** (`SF Mono` / `ui-monospace`): progressive step markers, HLC stamps, and sync telemetry.
  3. **Editorial Milestone Accent** (`Doppio One` / `Georgia` italic): contextual milestone tag and zero-guilt velocity markers.
  4. **Body & Micro-Directives** (`DM Sans` Regular 400): secondary execution context and setup instructions.
- Provide production-ready design tokens, tactile containers (`#20201F` dark / `#ECE6DA` light), pill action triggers, and friction-controlled modal overlays.

## When to use me
Use this skill when:
- Designing or coding any UI in Worldline (Stackelberg active directive canvas, the control panel, evening audit, BIP-39 recovery screen).
- Styling the 9:16 desktop viewport container (`420px × 747px`), preventing maximization and full-screen layouts.
- Implementing the single-command canvas, the task ledger, the staged plan preview, or the difficulty-calibration track.
- Reviewing PRs for contrast compliance, zero-guilt feedback styling, and local-first typography performance.

---

## 1. Design Philosophy
- **Stackelberg Single-Directive Canvas:** No scrollable task lists, kanban boards, or calendar backlogs. Screen real estate is reserved for **exactly one directive at a time**.
- **Fixed 9:16 Vertical Discipline:** The desktop client is an ambient handheld companion (`420px × 747px`). It never maximizes or takes over the screen.
- **Calm & Tactile Dark Default:** Deep matte-graphite canvas (`#131312`), elevated directive container (`#20201F`), elevated hover (`#2A2A29`), and linen text (`#E5E2E0`). Pure `#000000` and `#FFFFFF` are prohibited.
- **Focal Coral Accent:** `#E26D52` is strictly reserved for the active step marker, primary action beacons, and cryptographic security badges—never applied as a surface background. (The active-timer pulse used to be the headline use; the session timer was removed 2026-09-26, so the active step badge and the create-goal button carry the accent now.)
- **Anti-Guilt Objective UI:** Skipped or demoted tasks never display red alarms or broken streaks. Velocity adjustments display neutral slate and sand tones resembling a navigational GPS recalculating a route.

---

## 2. Typography Specification

**12px is the floor. Nothing in this product is rendered below it.** This is
the single most load-bearing rule in this section, and it is stated first
because the previous revision of this file was already below its own floor:
the stylesheet had drifted to 9px and 10px in twenty-eight declarations, and
a page ended up stacking three label tiers at 11px / 11px / 12px that
differed from each other by one pixel of letter-spacing. At 420px wide those
are not labels, they are texture. If a new element seems to need 11px, it
needs *less content*, not smaller type.

| Token | Size | Family | Weight | Role |
|---|---|---|---|---|
| `--t-display` | 30px | `Doppio One`, `Georgia`, serif | 400 | Hero: boot wordmark, empty-state statement, seed-phrase title |
| `--t-page` | 24px | `Doppio One`, `Georgia`, serif | 400 | Page title, centred |
| `--t-directive` | 24px | `DM Sans`, sans-serif | 700 | **The** active directive |
| `--t-title` | 17px | `DM Sans`, sans-serif | 600 | Card / section / modal heading |
| `--t-lead` | 16px | `DM Sans`, sans-serif | 500–600 | A value worth reading; primary CTA |
| `--t-body` | 15px | `DM Sans`, sans-serif | 400 | Body copy, nav rows, model display names |
| `--t-caption` | 13px | `DM Sans`, sans-serif | 400–500 | Notes, hints, field labels, empty states |
| `--t-mono-lead` | 13px | `ui-monospace` | 500 | **Ids you read character by character** |
| `--t-mono` | 12px | `ui-monospace` | 500 | Telemetry, counts, HLC, chips, keys |

Three families, four weights, one floor. `--t-mono-lead` exists separately
from `--t-mono` for one reason: a model id is *the value that gets sent*, and
it is compared character by character against a catalog. Rendering it at
the telemetry size made `nvidia/nemotron-3.5-lightning:free` a squeezed
smudge. A seed word and a relay URL are the same class of thing — an exact
string — and get the same treatment.

### Group identity comes from size, not from case

**Section headings are 17px sans, sentence case. They are not uppercase
monospace micro-caps.** This reverses the previous specification, which
called for 10–11px mono caps, and it is the largest single legibility win
in the system. Uppercase small mono was doing a job — making a label
distinguishable from the content beneath it — by a means that does not
survive a 420px window.

Identity now comes from *size and weight relative to what is labelled*,
which survives any viewport. `.wl-section-title` is 17px/600 in
`--wl-text-primary` against 15px/500 body and 13px/400 notes: the ladder is
obvious without a single letterform trick.

The mono eyebrow survives, at the 12px floor, for genuine telemetry groups
only — HLC stamps, counts, a `MOCK DATA` marker, a `SEALED` chip. It is
never a field label and never a section heading.

### Three text steps, not four

| Token | Dark | Light | Contrast on the card |
|---|---|---|---|
| `--wl-text-primary` | `#E5E2E0` | `#1D1C13` | 12.6:1 |
| `--wl-text-secondary` | `#CAC6BC` | `#4A4740` | 9.9:1 |
| `--wl-text-muted` | `#949087` | `#5E5A53` | 5.4:1 |

Every step clears 5:1 against `--wl-surface-card`, which is what makes the
small sizes usable. **A group label must be quieter than the rows it
heads** — `--wl-text-muted`, not primary. There was a fourth step
(`--wl-text-tertiary`) that was referenced by thirteen rules and defined in
none of them; a declaration naming an undefined custom property is invalid
at computed-value time and silently inherits, so those labels rendered
*brighter* than the content beneath them. The token no longer exists, and
the three-step ladder is short enough that a fourth step has nowhere
honest to go.

### CSS Configuration
```css
:root {
  --font-sans: 'DM Sans', -apple-system, BlinkMacSystemFont, sans-serif;
  --font-mono: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
  --font-serif: 'Doppio One', Georgia, 'Times New Roman', serif;

  --t-display: 30px;  --t-page: 24px;  --t-directive: 24px;
  --t-title: 17px;    --t-lead: 16px;  --t-body: 15px;
  --t-caption: 13px;  --t-mono-lead: 13px;  --t-mono: 12px;
}

/* The one thing in the app at --t-directive. */
.wl-directive-title {
  font-family: var(--font-sans);
  font-weight: 700;
  font-size: var(--t-directive);
  line-height: 1.3;
  letter-spacing: -0.025em;
  color: var(--wl-text-primary);
}

/* A section heading. NOT uppercase, NOT mono. See §2. */
.wl-section-title {
  font-family: var(--font-sans);
  font-size: var(--t-title);
  font-weight: 600;
  line-height: 1.3;
  letter-spacing: -0.02em;
  color: var(--wl-text-primary);
}

/* Telemetry only. Never a field label, never a section heading. */
.wl-eyebrow {
  font-family: var(--font-mono);
  font-weight: 500;
  font-size: var(--t-mono);
  letter-spacing: 0.09em;
  text-transform: uppercase;
  color: var(--wl-text-muted);
}
```

---

## 3. Color Palette & Token System

### Dark Canvas (Primary Default)
| Token | Hex | Role & Application |
|---|---|---|
| `--bg-dark-base` | `#131312` | 9:16 Canvas viewport, matte warm slate |
| `--bg-dark-card` | `#20201F` | Primary Directive container card |
| `--bg-dark-elevated` | `#2A2A29` | Chip badges, secondary controls, inputs |
| `--bg-cream-primary` | `#DAD5C7` | Primary action (the compose CTA, a filled progress bar) |
| `--accent-coral` | `#E26D52` | Active step marker, primary action beacon, security ring |
| `--text-dark-primary` | `#E5E2E0` | Command titles, high-contrast labels |
| `--text-dark-secondary` | `#949087` | Execution context, time indicators |
| `--text-dark-inverse` | `#1D1C13` | Contrast text inside cream CTA buttons |
| `--border-subtle` | `rgba(229, 226, 224, 0.08)` | Frame borders, divider rules |

### Light Canvas (Warm Parchment Variant)
| Token | Hex | Role & Application |
|---|---|---|
| `--bg-light-base` | `#F4F0E8` | Parchment canvas background |
| `--bg-light-card` | `#ECE6DA` | Active directive container |
| `--bg-light-elevated` | `#E0DAD0` | Secondary triggers and HUD counters |
| `--bg-charcoal-primary` | `#1D1C13` | Primary execution CTA in light mode |
| `--accent-coral` | `#D8583B` | Focal active-step indicator |
| `--text-light-primary` | `#1D1C13` | Deep slate-charcoal command titles |
| `--text-light-secondary` | `#68645C` | Execution subtext and helper guides |
| `--border-subtle` | `rgba(29, 28, 19, 0.10)` | Divider rules on parchment |

### Full Design Token Reference (YAML Source)
```yaml
name: Worldline Design System
colors:
  surface: '#131312'
  surface-dim: '#131312'
  surface-bright: '#393938'
  surface-container-lowest: '#0e0e0d'
  surface-container-low: '#1c1c1b'
  surface-container: '#20201f'
  surface-container-high: '#2a2a29'
  surface-container-highest: '#353533'
  on-surface: '#e5e2e0'
  on-surface-variant: '#cac6bc'
  inverse-surface: '#e5e2e0'
  inverse-on-surface: '#31302f'
  outline: '#949087'
  outline-variant: '#49473f'
  surface-tint: '#cbc6b9'
  primary: '#f7f1e3'
  on-primary: '#323027'
  primary-container: '#dad5c7'
  on-primary-container: '#5f5c51'
  inverse-primary: '#615e53'
  secondary: '#c8c6c2'
  on-secondary: '#31302d'
  secondary-container: '#474743'
  on-secondary-container: '#b7b5b0'
  tertiary: '#ffeeeb'
  on-tertiary: '#631000'
  tertiary-container: '#ffc9bd'
  on-tertiary-container: '#9f3c25'
  error: '#ffb4ab'
  on-error: '#690005'
  error-container: '#93000a'
  on-error-container: '#ffdad6'
  primary-fixed: '#e7e2d4'
  primary-fixed-dim: '#cbc6b9'
  on-primary-fixed: '#1d1c13'
  on-primary-fixed-variant: '#49473c'
  secondary-fixed: '#e5e2dd'
  secondary-fixed-dim: '#c8c6c2'
  on-secondary-fixed: '#1c1c19'
  on-secondary-fixed-variant: '#474743'
  tertiary-fixed: '#ffdad2'
  tertiary-fixed-dim: '#ffb4a3'
  on-tertiary-fixed: '#3d0600'
  on-tertiary-fixed-variant: '#822712'
  background: '#131312'
  on-background: '#e5e2e0'
  surface-variant: '#353533'
typography:
  display-hero:
    fontFamily: 'Doppio One, Georgia, serif'
    fontSize: '38px'
    fontWeight: '400'
    lineHeight: '44px'
    letterSpacing: '-0.025em'
  display-directive:
    fontFamily: 'DM Sans, Outfit, sans-serif'
    fontSize: '26px'
    fontWeight: '700'
    lineHeight: '34px'
    letterSpacing: '-0.03em'
  body-lg:
    fontFamily: 'DM Sans, Outfit, sans-serif'
    fontSize: '15px'
    fontWeight: '400'
    lineHeight: '22px'
    letterSpacing: '-0.01em'
  mono-telemetry:
    fontFamily: 'ui-monospace, SF Mono, Menlo, monospace'
    fontSize: '12px'
    fontWeight: '500'
    lineHeight: '16px'
    letterSpacing: '0.04em'
rounded:
  sm: '0.5rem'
  DEFAULT: '0.75rem'
  md: '1.25rem'
  lg: '1.75rem'
  xl: '2.25rem'
  full: '9999px'
```

---

## 4. UI Component Architecture

### A. Floating Controls
Overlays the canvas instead of sitting in a bar above it. A full-width header strip fought the core premise — ONE directive, no chrome — and squeezed the empty state into a letterbox, so the bar was removed (2026-09-26). The menu is a floating circular control (top-left), raised off the surface with a soft shadow.

**The canvas has exactly one floating control.** The sync control that used to float opposite the menu (2026-09-27) was a 34px circle that reported nothing until tapped; sync is a *settings* concern, not a canvas one, so it moved to the Sync section of Settings as a full-width row that also shows what the last cycle did. Nothing became unreachable — sync status and the Evening audit were already in the `Ctrl+,` telemetry drawer.

**The menu opens the control panel, not a file list.** Two buttons in an
otherwise empty 273px sheet was the old drawer, and the void above them
was the design: a menu that only routes to other menus contradicts a
product whose premise is that you should never be browsing. The panel
now answers "where am I?" without leaving the canvas — a primary action,
a **System** group (Entropy Log, Trajectory), an **Active worldlines**
readout, and Settings. The worldlines are a *readout*: no hover state, no
pointer cursor, because a tappable row with no detail page behind it is a
dead affordance.

**The panel is full height — top edge to bottom edge of the frame.**
312px wide, `height: 100%`, anchored top-left. A drawer that stops halfway
reads as a card that failed to open, not as a deliberate choice: the canvas
stays visible beneath it and there is no way to tell from the outside whether
the tap missed or the surface broke. This was tried the other way (hugging
content, `max-height: 100%`) and the user rejected it in the first build
that shipped.

Full height creates a real problem — a 747px sheet whose content ends at
400px has 350px of nothing in it — and the fix is **not** to shrink the
panel. The slack goes to `.wl-nav-worldlines { flex: 1 }`, so the empty
space lands INSIDE the readout region, where a list is expected to have
room, and the fixed things (the primary action, the two System rows) keep
their rhythm at the top with Settings pinned to the floor where a thumb
expects it. `justify-content: space-between` on the scroll region is the
wrong tool: it spreads the gaps evenly and pulls the primary action away
from the top of the sheet.

This is the fourth layout this panel has had. The first pinned two buttons
to the floor behind a `flex: 1` spacer, where the spacer *was* the layout.
The second centred the groups to "balance" a stretched sheet. The third
hugged the content. All three manufactured a shape instead of reporting
content, and the two voids were visible in every one.

**Group labels in the panel are dimmer than the rows they head.** Mono
eyebrow at the 12px floor, in `--wl-text-muted`, against 15px/500 rows in
`--wl-text-primary`. When the group label is the *loudest* thing in the
panel, the panel has no hierarchy — which is precisely what the dead
`--wl-text-tertiary` bug produced.

```html
<div class="wl-float-layer">
  <button class="wl-circle-btn" aria-label="Open navigation">
    <svg class="wl-icon-menu wl-icon" view_box="0 0 18 18" fill="none"
         stroke="currentColor" stroke_width="1.6" stroke_linecap="round"
         aria-hidden="true"><path d="M3 5h12M3 9h12M3 13h12" /></svg>
  </button>
</div>
```

**Icons are inline SVG, never font glyphs — there are no exceptions left.**
A glyph's weight, spacing, and (on many Linux desktops) its colour are
chosen by the font stack, not by us. Every icon strokes with `currentColor`
so it inherits the theme tokens for free, is sized by CSS only (`.wl-icon` +
a per-icon modifier, never a `width`/`height` attribute), and is
`aria-hidden` because the control around it already carries the real
`aria-label`.

This section previously excepted three glyphs — `✚` on the control panel's
primary action, `›` on every trailing row, `✕` in the choice sheet — on the
reasoning that a plus is "unambiguous at 20px". That was true of their
*shape* and false of their *rendering*: the same three marks sat at three
different sizes and baselines across three sheets because the font stack
decided, which is exactly the drift the SVG set exists to remove. All three
are paths now (`IconNew`, `IconChevron`, `IconClose`).

The one remaining typographic mark is the evening check-in's `●` / `◐` / `○`.
That one is a *state* rendered as one figure at three fill levels, and a
half-filled circle is a typographic idiom with no universally recognised
drawn equivalent. It is at 18px, in `--wl-text-secondary`, and it is the
only glyph in the app.

**The settings mark is a gear**, drawn as one filled path with `fill-rule="evenodd"`: the outer sub-path is the notched rim, the trailing circle sub-path is the hub hole, and evenodd knocks it out. It is therefore the one icon that is `fill: currentColor` while the others are stroked. Its geometry was measured off a reference mark (outer r 8.6 on a 24 grid, hole r 3.4 ≈ 0.40 of outer, root r 7.0, eight teeth at ±15°) after two attempts that were rendered and rejected: a stroked ring-plus-teeth version reads as a **sun**, and deeper/wider notches read as a **ship's wheel**. Rejected-by-rendering is the normal path for icon geometry here — none of these three could be judged by reading the path data.

```html
<div class="wl-float-layer">
  <button class="wl-circle-btn" aria-label="Open navigation">
    <svg class="wl-icon-menu wl-icon" view_box="0 0 18 18" fill="none"
         stroke="currentColor" stroke_width="1.6" stroke_linecap="round"
         aria-hidden="true"><path d="M3 5h12M3 9h12M3 13h12" /></svg>
  </button>
</div>
```

**There is one circular control, and it is defined once.** `.wl-circle-btn`
is the 38px raised circle used by the canvas menu *and* by the back chevron
on every secondary page. These were two separate 34px definitions with
different offsets, so the two controls that occupy the same corner on the
same screen did not line up — which is most of what "the hamburger looks
inconsistent" meant in practice. One class, both call sites.

The canvas's button carried a `wl-float-menu` modifier. It had no rule
anywhere in the stylesheet, and when one was finally written for it
(`position: static`) it changed nothing — `.wl-circle-btn` is already
`position: relative` and the control is never used inside a page header
(those go through `.wl-back`). **The modifier is deleted.** A class that
cannot change a declaration is a lie the next reader has to disprove, and
it sits in the markup looking like a specificity override that matters.

**The floating layer is positioned against the SCREEN, never the frame.**
`.wl-float-layer` is `position: absolute`, so its containing block is the
nearest positioned ancestor, and `.wl-canvas-screen` is what provides one on
the canvas. Without it the layer resolved against `#main` — the app frame —
and the measurement in the real WebKitGTK webview showed the cost: the
hamburger's rect was `x=16 y=12` in a viewport whose content box starts at
`x=18`, so the one control the canvas has sat 2px *outside* the content it
floats over and 8px above its top edge, because the layer's own padding was
being spent inside the frame's padding. It also meant the control's position
was decided by an ancestor that also hosts the drawer, the toast and the
error screen: any future `transform`, `filter` or `contain` on `#main` would
have moved the hamburger. A control belongs to the surface it floats over.

```css
.wl-canvas-screen { position: relative; }
.wl-float-layer {
  position: absolute; top: 0; left: 0; right: 0; z-index: 10;
  display: flex; align-items: flex-start; justify-content: flex-start;
  padding: 12px 0 0;           /* no horizontal padding: `left: 0` is already the content edge */
  pointer-events: none;        /* the layer is not a target; controls opt back in */
}
```

**`Escape` closes exactly one layer, and the layers have an order.** The
control panel is above the `Ctrl+,` telemetry drawer, which is above
nothing. This is data (`topmost_overlay` / `dismiss_topmost` in `app.rs`),
not a chain of `if`s, because it used to be implicit and wrong: the canvas
and the app root each ran an `Escape` handler and both fired on the same
bubbled keydown, so one press could open the bailout modal *and* dismiss a
panel. From outside that is indistinguishable from a menu that does not
open. `Ctrl+,` is likewise refused while another layer is up — the telemetry
drawer is a later sibling with a higher z-index, so summoning it over the
control panel buried the panel rather than failing visibly, which is the
same symptom again.

**The ladder is two deep, and the test says so.** The categorisation sheet
was its top rung until 2026-09-27, when the escape hatch was deleted. A
third layer added back without a rule for it fails
`escape_closes_exactly_the_topmost_layer` — which is the whole reason the
order is data.

```css
.wl-circle-btn {
  pointer-events: auto; position: relative;
  display: inline-flex; align-items: center; justify-content: center;
  width: 38px; height: 38px; flex-shrink: 0;
  border-radius: 50%;
  border: 1px solid var(--wl-border-subtle);
  background: var(--wl-surface-elevated);
  color: var(--wl-text-primary); line-height: 1; cursor: pointer;
  /* The raised look: elevated surface + soft drop shadow, so it reads as
     sitting above the content rather than embedded in it. */
  box-shadow: var(--wl-raise);
  transition: transform var(--duration-instant) var(--ease-tactile),
              background-color var(--duration-instant) var(--ease-tactile);
}
.wl-circle-btn:hover:not(:disabled) { background: var(--wl-surface-high); }
.wl-circle-btn svg { display: block; }
```

Two rules this section now encodes:

- **There is no session timer.** The `MM:SS` readout, its 1 Hz ticker, and the whole `TimerSession` state were removed 2026-09-26. The canvas shows no elapsed time at all; work is bounded by the directive, not by a clock. `elapsed_secs` survives only for sync freshness (`SYNCED · 42s ago`) in the telemetry drawer, which is not timer-shaped — do not reintroduce a timer from it.
- **`.wl-hud` and `.wl-chip` each have exactly one definition.** Both were
  declared twice with different geometry, and the second silently won
  everywhere — which is why the "sealed" key chip in Settings and the
  "blocked" chip in the Entropy Log were one class at two sizes. A duplicate
  selector is not a style, it is a coin flip.

### A2. Centred Page Header (secondary screens)
Goal creation and Settings open with the same fixed header: a **centred** title with a floating back chevron in the left gutter. The header sits outside the scroll region with `flex-shrink: 0`, so on a long page the way back never scrolls out of reach.

**The back button must be `position: absolute`, never a flex child.** As a 34px flex sibling it pushes the title permanently right of the viewport axis, and no `justify-content` value fixes that without also centring the button. Symmetric side padding is what keeps the centred text on-axis while reserving the button's space.

```html
<div class="wl-page-head">
  <button class="wl-back" aria-label="Back to the line">
    <svg class="wl-icon-back wl-icon" view_box="0 0 18 18" fill="none"
         stroke="currentColor" stroke_width="1.6" stroke_linecap="round"
         stroke_linejoin="round" aria-hidden="true">
      <path d="M11 3.5 5.5 9l5.5 5.5" /></svg>
  </button>
  <div>
    <h1 class="wl-page-title">Settings</h1>
  </div>
</div>
```

```css
.wl-page-head {
  position: relative; display: flex; justify-content: center;
  text-align: center; flex-shrink: 0;
  /* 40px gutter: the 34px control plus 6px clearance, mirrored so the
     centred title stays on the viewport axis. */
  padding: 2px 40px 16px;
}
.wl-back { position: absolute; left: 0; top: 2px; /* …circular control… */ }
.wl-page-title { font-family: var(--font-serif); font-size: 24px; }
```

Two rules this section encodes:

- **A page title is one plain serif line — no sub-heading, no accent.** Both titles ("Settings", "State the Objective") are the same weight, colour, and case treatment. A deck under a short heading restated the heading rather than adding anything, and `.wl-page-sub` was deleted with it. **The coral accent does not go in a title.** It was briefly applied to the word "objective", which made that heading read as a different kind of object from every other screen and spent the beacon on a decorative word rather than on something live. `.wl-italic-accent` remains correct for the *editorial* headings it was designed for — the dormant line, the seed-phrase title.
- **`--font-serif` titles are 24px, not 26px**: the centred column is 40px narrower per side, and "State the Objective" is the longest title in the app.

### A3. Compose Screen (goal creation)
Goal creation is a page of its own kind: **not** a settings form. It borrows the canvas's premise — ONE thing, no chrome — rather than the four-cards-and-a-save-button shape. One bare free-text field owns the screen, and the architect names the goal from what the user typed.

```html
<div class="wl-page-bar">
  <button class="wl-back" aria-label="Back to the line"><!-- chevron svg --></button>
  <select class="wl-horizon" aria-label="Target date">
    <option>No date</option><option>1 week</option><option>1 month</option>
    <option>3 months</option><option>6 months</option><option>1 year</option>
  </select>
</div>
<h1 class="wl-page-title">State the Objective</h1>
<textarea class="wl-compose" rows="1" maxlength="4000" autofocus
          placeholder="What do you want to accomplish?"></textarea>
<div class="wl-actions-row">
  <button class="wl-btn-primary">Generate</button>
  <button class="wl-btn-ghost">Create manually</button>
</div>
```

Four rules this section encodes:

- **The field is bare — no card, no resting border.** A box around it fights the "this IS the page" reading. Affordance comes from the caret, a muted placeholder, and a coral hairline that exists only while focused. That hairline is the *only* coral on the screen.
- **`Enter` inserts a newline. Only the buttons submit.** There is deliberately no submit chord: the field is multi-line, so `Enter` must stay a newline, and a shortcut on a compose screen invites firing it mid-thought.
- **`.wl-page-bar` exists because the title and the horizon pill cannot share a row.** The title is ~230px wide inside a 304px padded box, so an absolutely-positioned pill in the right gutter overlays its last ~50px. The bar also resets `.wl-back` to `position: static` — left absolute (as it is in `.wl-page-head`, where it overlays a centred title) it leaves the flow, the pill becomes the only in-flow child, and `space-between` pins it to the **left** edge.
- **The horizon is a pill, not a date input.** "By when?" is a horizon question far more often than a calendar one, and a native date widget reads as a different object from everything else on the page. It stays a real `<select>` (restyled) so keyboard and screen-reader behaviour are the platform's.

### B. The Stackelberg Single Directive Card
The focal heart of the application. Presents only one directive.

```html
<main class="wl-directive-container">
  <div class="wl-directive-card">
    <div class="wl-directive-step-badge">Phase 1 of 2 · 25 Minutes</div>
    <h1 class="wl-directive-title">Draft the initial schema migrations for SQLite persistence</h1>
    <p class="wl-directive-instruction">
      Define tables for <code class="wl-code">identity_config</code> and <code class="wl-code">crdt_outbox</code>.
      Keep primary keys as deterministic UUIDv4. Do not wire API sync yet.
    </p>
    <div class="wl-progress-track">
      <div class="wl-progress-bar" style="width: 50%;"></div>
    </div>
  </div>
</main>
```

```css
.wl-directive-container {
  flex: 1; display: flex; flex-direction: column;
  justify-content: center; padding: 12px 0;
}
.wl-directive-card {
  background-color: var(--wl-surface-card);
  border: 1px solid var(--wl-border-strong);
  border-radius: var(--wl-radius-card);
  padding: 28px 24px;
  box-shadow: 0 4px 24px rgba(0, 0, 0, 0.4);
}
.wl-directive-step-badge {
  font-family: var(--font-mono); font-size: var(--t-mono); font-weight: 500;
  text-transform: uppercase; color: var(--wl-accent-coral);
  letter-spacing: 0.05em; margin-bottom: 14px;
}
.wl-directive-title {
  font-family: var(--font-sans);
  font-size: var(--t-directive); font-weight: 700;
  line-height: 1.3; letter-spacing: -0.025em;
  color: var(--wl-text-primary); margin: 0 0 14px 0;
}
.wl-directive-instruction {
  font-size: var(--t-body); line-height: 1.6;
  color: var(--wl-text-secondary); margin: 0 0 24px 0;
}
.wl-code {
  font-family: var(--font-mono);
  background: var(--wl-surface-elevated);
  padding: 2px 6px; border-radius: 4px;
  color: var(--wl-text-primary); font-size: var(--t-mono);
}
.wl-progress-track {
  width: 100%; height: 4px;
  background-color: var(--wl-surface-high); border-radius: 2px; overflow: hidden;
}
.wl-progress-bar {
  height: 100%; background-color: var(--wl-cta-bg); transition: width 200ms ease;
}
```

### C. The Canvas Is Read-Only, and the Ledger Is the Only Writer

**The canvas carries no controls at all** (2026-09-27, and this is the
second time). The footer that held "Complete Directive" and "Bailout /
Blocked" went first; then the two keyboard gestures that reached the same
state changes went too — `⌘+Enter` and `Escape`. The menu is now the only
thing on the surface, and there is **no keydown handler on the canvas root
at all**: not a hidden one, not a documented one. A task is worked here and
resolved on the ledger, one hamburger row away.

**The escape hatch is gone, and it is not coming back as a shortcut.**
PRD §5.3 is superseded. With no trigger left, so went `bail_out`,
`BailoutReason`, `RecoveryAction`, the categorisation sheet, and the reason
column on the ledger. What is lost is real and is recorded in PRD delta 225:
the frictionful sheet was the mechanism that made a stall *informative*, and
without it the system can no longer say *why* something did not happen. It
can only say that it did not.

```html
<!-- removed; kept as the record of what the canvas no longer renders -->
<footer class="wl-actions">
  <button class="wl-btn-primary">
    <span>Complete Directive</span>
    <kbd class="wl-kbd">⌘↵</kbd>
  </button>
  <button class="wl-btn-escape">
    <span>Bailout / Blocked</span>
    <kbd class="wl-kbd-subtle">Esc</kbd>
  </button>
</footer>
```

`.wl-btn-primary` is the CTA everywhere — compose, plan preview, dormant,
check-in. `.wl-btn-escape` and `.wl-kbd` are **dead classes** and were
deleted with the sheet; do not reintroduce them.

The one rule this section replaces: **a control must have an engine
transition behind it, or it does not ship.** The ledger's tick is the only
verb `Engine::mark_complete` implements, so the ledger has exactly one
control. There is deliberately **no cross** on a task — "not done" would
have to mean something, and the honest candidate needs a category to be
useful, which is the escape hatch under a new name.

```css
/* Warm Cream Primary CTA */
.wl-btn-primary {
  width: 100%; height: 52px;
  background-color: var(--wl-cta-bg); color: var(--wl-text-inverse);
  font-family: var(--font-sans-directive); font-size: 15px; font-weight: 600;
  border-radius: var(--wl-radius-btn); border: none; cursor: pointer;
  display: flex; justify-content: center; align-items: center; gap: 10px;
  transition: background-color 120ms ease, transform 120ms ease;
}
.wl-btn-primary:hover:not(:disabled) {
  background-color: var(--wl-cta-hover); transform: translateY(-1px);
}
.wl-btn-primary:disabled { opacity: 0.5; }
```

### C2. The Task Ledger (the Entropy Log page)

Every task, grouped by goal, with **one** control per row. This is the only
surface in the product that resolves anything, and it is a list — which is
the one place the "no scrollable list of future tasks" rule is narrowed
rather than obeyed (PRD delta 227).

Two rules this section encodes:

- **`blocked_by` is the field that earns the page.** A task sitting in the
  queue with nothing visibly wrong with it is indistinguishable from a
  stalled app. The row names the unfinished task it is waiting on, in
  words: *"waiting on Draft the outline"*. A chip or a count would say
  nothing a person can act on.
- **A finished task is quieter, never struck through.** A line through text
  is a verdict, and this product does not deliver them. The tick's pressed
  state is cream, **not coral** — coral is a beacon for something live, and
  a finished task is neither live nor an alarm.

### C3. The Plan Preview (Generate does not write)

Generate is **staged**: it fetches a plan, shows it, and writes nothing
until Commit. So the preview is a page like any other, and backing out of
it is free.

- **The graph is a vertical spine, not a force layout.** At 420×747 a
  force-directed graph is unreadable and a layered one gets worse fast —
  five milestones of four tasks is twenty nodes and fifteen edges with no
  room to draw either. The ORDER is the graph: a rail with a dot per task,
  filled for one that can run and hollow for one that is waiting.
- **An edge is said in words on the node it points into** — *"after Draft
  the outline"* — not drawn as a dashed rail, which says nothing extra.
- **Every field is editable in place.** A correction costs nothing; a
  re-generate costs a billable call and throws away the four things you were
  happy with.
- **"Nothing is saved until you commit" is stated on the page.** It is the
  one promise the staging makes, and a promise the user cannot verify from
  the outside is one they should be told about.
- **A generated plan is never silently reshaped.** Whatever the shell
  repaired or the seeded fallback replaced is printed above the plan, with
  the count. A plan you approved a preview of must not differ from the one
  that gets written without saying so.

### D. Zero-Knowledge BIP-39 Seed Vault (Onboarding Card)
Tactile 12-word recovery display during cryptographic account initialization.

```html
<section class="wl-seed-vault">
  <div class="wl-seed-header">
    <h2 class="wl-serif-title">Secret <span class="wl-italic-accent">recovery</span> phrase</h2>
    <p class="wl-seed-sub">Write these down in exact sequence. Worldline has no email servers to recover them.</p>
  </div>
  <div class="wl-seed-grid">
    <div class="wl-seed-token"><span class="wl-seed-num">01</span> beacon</div>
    <div class="wl-seed-token"><span class="wl-seed-num">02</span> orbit</div>
    <div class="wl-seed-token"><span class="wl-seed-num">03</span> silence</div>
    <div class="wl-seed-token"><span class="wl-seed-num">04</span> dynamic</div>
    <div class="wl-seed-token"><span class="wl-seed-num">05</span> marble</div>
    <div class="wl-seed-token"><span class="wl-seed-num">06</span> drift</div>
    <div class="wl-seed-token"><span class="wl-seed-num">07</span> lattice</div>
    <div class="wl-seed-token"><span class="wl-seed-num">08</span> kinetic</div>
    <div class="wl-seed-token"><span class="wl-seed-num">09</span> harbor</div>
    <div class="wl-seed-token"><span class="wl-seed-num">10</span> canyon</div>
    <div class="wl-seed-token"><span class="wl-seed-num">11</span> velvet</div>
    <div class="wl-seed-token"><span class="wl-seed-num">12</span> anchor</div>
  </div>
</section>
```

```css
.wl-seed-vault { display: flex; flex-direction: column; gap: 20px; }
.wl-serif-title {
  font-family: var(--font-serif); font-size: var(--t-display); font-weight: 400;
  margin: 0 0 8px 0; color: var(--wl-text-primary);
}
.wl-italic-accent { font-style: italic; color: var(--wl-accent-coral); }
.wl-seed-sub { font-size: var(--t-caption); line-height: 1.5; color: var(--wl-text-muted); margin: 0; }
.wl-seed-grid { display: grid; grid-template-columns: repeat(2, 1fr); gap: 8px; }
.wl-seed-token {
  background-color: var(--wl-surface-card); border: 1px solid var(--wl-border-subtle);
  border-radius: var(--wl-radius-pill); padding: 10px 14px;
  /* The 12 seed words are the ONE thing in this product that must be
     transcribed by hand, so they get --t-mono-lead rather than the
     telemetry size. */
  font-family: var(--font-mono); font-size: var(--t-mono-lead);
  color: var(--wl-text-primary);
  display: flex; align-items: center; gap: 10px;
}
.wl-seed-num { color: var(--wl-text-muted); font-size: var(--t-mono); }
```

### E. The Value Row
Every "this is a value chosen from a list, not typed into" control is this
one component. It replaced three different-looking controls in Settings —
and one of the three was a native `<select>` that WebKitGTK painted as a
**near-white field carrying #E5E2E0 text** inside a graphite card. It
shipped twice: once "fixed" with `color-scheme: dark` plus
`appearance: none`, and once restyled. Neither declaration reliably reaches
the widget.

**Settings now contains no native form widget at all.** The compose
screen's target-date horizon is the app's last `<select>`, and it earns the
exemption because it is a small closed control with no catalog to search and
no history of painting itself white.

```html
<button class="wl-value-row">
  <span class="wl-value-row-label">Architect model</span>
  <span class="wl-value-row-value">anthropic/claude-sonnet-5</span>
  <span class="wl-value-row-meta">1.0M context</span>
  <span class="wl-value-row-go"><svg class="wl-icon wl-icon-chevron" …></svg></span>
</button>
```

Three rules:

- **The value is 13px mono, not 12px.** A model id is *the value that gets
  sent* and is compared character by character against a catalog. At the
  telemetry size with `word-break: break-all` it was one squeezed line; at
  `--t-mono-lead` with a two-line clamp it is readable. A value the user
  must verify does not get the size reserved for numbers they merely scan.
- **The meta line is a fact or it is absent.** No placeholder, no `—`, no
  "unknown". A rendered placeholder sits under every unset slot and trains
  the eye to skip the line.
- **The label is `--wl-text-secondary` and the value is
  `--wl-text-primary`.** The label introduces; it does not compete.

### F. Settings Commit Model
**Every field commits on `change` — blur or Enter, never on `input`.** There
is no Save button and there is no "appearance switches save themselves"
footnote, because that footnote was an admission that the two-path model was
wrong.

`settings_save` overwrites the *whole* settings row, so a commit cannot be a
patch. The rule is: **send the last shell-confirmed state with only the
field the user just changed applied.** Flipping the theme therefore cannot
save a half-typed relay URL, and picking a model cannot save a half-typed
API key. The behaviour lives in one pure function (`commit_payload`) and is
tested per-field.

One field is deliberately wider than its name: changing the *provider* also
commits both cleared model ids, because a model id is provider-specific and
a stale one produces a 404 the user has no way to interpret. Both the draft
and the payload must clear them, or the rows clear on screen while the old
ids stay in the database.

A failed commit reverts the draft to the last confirmed state and says so
(`NOT SAVED — …`). Showing a value the database does not hold is worse than
an error message.

---

## 5. Implementation Rules

### Do:
- **Lock the Viewport:** Constrain the desktop UI to `420px × 747px` (`resizable: false`, `maximizable: false`). Never allow layout stretch on widescreen monitors.
- **Never render below 12px.** The whole point of §2 is the floor. If something does not fit at 12px, it has too much text — shorten it, do not shrink it.
- **Enforce the Single-Command Rule:** Only one active directive container may be rendered in the DOM at any given execution cycle.
- **Emphasize Primary CTA Contrast:** The execution button (`.wl-btn-primary`) must always be rendered in warm cream `#DAD5C7` to act as an unequivocal behavioral magnet.
- **Render Telemetry in Monospace:** All durations, HLC sequence stamps, and sync counts must use `var(--font-mono)` at `var(--t-mono)` to prevent tabular jitter. Exact strings the user must verify — model ids, seed words, relay URLs — use `--t-mono-lead` instead. (Countdowns no longer exist — the session timer was removed 2026-09-26 — but the rule stands for whatever numeric readout comes next.)
- **Every control needs an engine transition behind it, or it does not ship.** This replaces "require confirmation on escape hatch", which required a feature that no longer exists. A tap target for something the engine cannot do is a control that lies, and it is why the ledger has a tick and no cross: `Engine::mark_complete` is the only verb, so a tick is the only honest control.
- **A long wait is always Cancel-able and always names its stage.** The Generate button used to read "Planning…" for two minutes with no way out and no idea what it was doing, which is indistinguishable from a hang. Two rules came out of it: the button label names the **command actually being awaited** (never a percentage, never invented progress — this webview has no event channel), and a Cancel sits beside it while a call is in flight.
- **Use switches for boolean preferences.** Theme is `.wl-switch[role=switch]` with `aria-checked`, not a `<select>` and not a label-bearing button. It applies **and persists immediately** — a theme toggle that only takes effect on save is a broken control, because you cannot evaluate a theme you are not allowed to see. (The window-pin switch is gone with the feature; PRD delta 175.)
- **Every settings field commits on `change`, and every commit is scoped to one field.** See §4.F. A commit sends the last shell-confirmed state with one field applied — never the whole draft, and never the whole row.
- **Switch state colours are absolute, not theme-derived.** The switch encodes "which theme is selected", not "what is currently rendered"; flipping its tokens with `data-theme` would invert the control the instant it took effect.
- **The control panel is a readout, not a browser.** Active worldlines rows carry no hover state and no pointer cursor. Do not make them tappable without a goal-detail page to land on.
- **An inert number must say it is inert.** `estimate_adjustment` is observed and not applied (PRD delta 33); every surface showing it repeats that. A figure that looks like a setting but changes nothing is the one thing this system will not ship.
- **Never fabricate a read-out under the mock.** Every `dx serve` response is canned. Anything the panel renders from a command says `· MOCK` when `transport()` reports the mock, as the model catalog already does (PRD delta 148).

### Don't:
- **Never display a scrollable list of future tasks on the home screen:** the canvas shows exactly one directive and resolves nothing. The **ledger** (the Entropy Log page) is the one place a task list exists, because a person opens it deliberately to answer "what is outstanding and what am I done with" — and its rows name what each unfinished task is waiting on, which is the fact a bare list cannot give. PRD delta 227.
- **Never use streaks or red failure states:** Missing a directive is a velocity adjustment event, never an alarm. Avoid punitive UI red `#FF0000`.
- **Never use pure white (`#FFF`) or deep black (`#000`):** Use the specified slate canvas (`#131312`) and muted text tones (`#E5E2E0` / `#CAC6BC`). This holds for switch knobs and tracks too.
- **No decorative background illustrations or icons:** Maintain tactile, terminal-level discipline. Avoid extraneous emojis and marketing illustrations. UI icons are functional inline SVG (see §4.A); emoji are not UI iconography.
- **Never put a control back in the canvas chrome for convenience.** Sync moved to Settings deliberately, the Complete/Bailout footer was removed, and the two canvas shortcuts went with the escape hatch; the canvas keeps exactly one floating control and **no keydown handler at all**.
- **Never split a compose field into a title field plus a details field.** The user types intent in their own words and the architect names the goal; see §4.A3. Corollary: a goal title derived from a raw fragment must be clipped before it reaches `create_goal`, because the manual path seeds the first directive with that title verbatim.
- **Never accent a page title.** Page titles are one plain serif line in `--wl-text-primary`. The coral beacon belongs to the active step badge, the create-goal button, and cryptographic security badges — not to a heading, and not to a decorative word inside one.
- **Never label a group with something smaller or louder than what it labels.** A 10px uppercase group heading above 15px rows is the exact failure that made the control panel read as inconsistent: the group became the loudest object on screen. Labels are 12px mono muted; content is 15px primary.
- **Never ship a native `<select>` you cannot fully style.** WebKitGTK paints its own closed state and `appearance: none` plus `color-scheme` do not reliably reach it. It shipped as a white box with linen text, twice. Settings has none.
- **Never style the same class twice.** A duplicate selector is not a style, it is a coin flip on which one the engine picks — and it is invisible until someone notices the two call sites disagree.
- **Never balance a panel by manufacturing empty space.** No `flex: 1` spacer, no centring to "fill" a stretched sheet. Content ends where it ends.
- **Never name a token you have not defined.** `var(--foo)` with no `--foo` is invalid at computed-value time and silently inherits. That one bug inverted the control panel's entire hierarchy, and it was invisible in the source because the stylesheet parsed cleanly.
- **Never ring a container.** The focus ring is for things a keyboard user can *operate*. `.wl-root` carries `tabindex="0"` and `autofocus` — the canvas took them to receive ⌘+Enter and Escape, and it no longer binds either, so they are now inherited boilerplate rather than load-bearing — and a bare `[tabindex]:focus-visible` selector (specificity 0,2,0) beat its own `outline: none` (0,1,0), drawing a coral ring around the entire 420×747 frame on every launch. The rule excludes `.wl-root` explicitly; do not "simplify" that back to a bare `[tabindex]`, which would also un-ring a future real custom control.
- **Never let a drawer stop short of its frame's edge.** The control panel is `height: 100%`, top to bottom. A sheet that hugs its content and ends mid-air leaves the canvas showing beneath it, and the user cannot tell whether the tap missed or the surface broke. When a full-height panel would leave a void, the fix is to give the slack to the region that can grow (the worldline readout), never to shrink the panel.

---

## 6. Performance Budget (Wasm & Local-First Targets)
1. **Zero Idle GPU Overhead:** No infinite glowing CSS animations or heavy `backdrop-filter: blur()` layers inside the 9:16 frame.
2. **Sub-16ms Frame Time:** State updates driven by the local SQLite/CRDT outbox must re-render the directive canvas in $<16\text{ ms}$.
3. **Hardware Storage Isolation:** API keys and BIP-39 mnemonic strings must pass directly from memory to Tauri Stronghold without serializing into temporary DOM dataset attributes.

---

## 7. Quick Start Checklist
1. Bind `:root` tokens: Canvas `#131312`, Card `#20201F`, CTA `#DAD5C7`, Accent Coral `#E26D52`. (The card token is `#20201F`; an older revision of this file said `#1C1C1B` while contradicting itself two sections earlier.)
2. Use only the nine type tokens in §2. **Nothing below 12px.** Section headings are `.wl-section-title` (17px sans, sentence case) — never small uppercase mono.
3. Restrict root viewport bounds to `420px × 747px` fixed portrait mode.
4. Wire typography: `Doppio One` for editorial headings and page titles, `DM Sans` (700 bold) for the active Stackelberg command, `ui-monospace` for telemetry and for exact strings.
5. Render exactly **one** primary directive card (`.wl-directive-card`) under exactly **one** floating control (`.wl-circle-btn`, the same class every back chevron uses).
6. The canvas binds **no** key handler. There are no canvas shortcuts left: `⌘+Enter` and `Escape` were both removed with the escape hatch, and the ledger's tick is the only way a task resolves.
7. Secondary pages centre their title over an absolutely-positioned back chevron. Every settings field commits on `change`, scoped to that one field; there is no Save button.
8. Any "choose a value" control is a `.wl-value-row` opening a bottom sheet. No native `<select>` in Settings.
9. **Before committing, diff the token list**: every `var(--x)` in the stylesheet must have a matching `--x:`. An undefined one is silent.

### Verifying a change
`scripts/dx.sh serve --port 1420` renders every screen against the real
`public/wl.css` in a browser, backed by the mock shell in
`public/invoke-shim.js` — no Tauri shell, no rebuild of the native crate.
Use it because the alternative is reviewing a design by reading a
stylesheet — which is how thirteen rules came to reference a token that
was defined nowhere, and how a cascade ordering mistake shipped a
non-full-height control panel. **Render it.**
