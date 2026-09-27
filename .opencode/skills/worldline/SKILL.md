---
name: worldline
description: Worldline Design System — Stackelberg single-directive 9:16 viewport (420px × 747px), tactile matte graphite tokens (#131312 / #20201F / #E26D52), calm zero-guilt velocity UI. Enforces directive command hierarchy, BIP-39 seed vault, and friction-controlled escape hatch.
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
- Implementing the single-command HUD, progressive disclosure steps, or the frictionful escape hatch modal.
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

| Role | Font Family | Weight | Style | Size / Line-Height / Tracking | Usage |
|---|---|---|---|---|---|
| Morning Brief / Hero | `Doppio One`, `Georgia`, serif | 400 | Normal | 38px / 1.15 / `-0.025em` | Morning greeting & audit titles |
| Hero Italic Accent | `Doppio One`, `Georgia`, serif | 400 | Italic | 38px / 1.15 / `-0.02em` | Editorial focus word ("*trajectory*") |
| Active Directive | `DM Sans`, `Outfit`, sans-serif | 700 | Normal | 24px / 1.30 / `-0.03em` | Core command ("Write 300 words") |
| Card / Sub-Header | `DM Sans`, `Outfit`, sans-serif | 600 | Normal | 16px / 1.35 / `-0.02em` | Milestone title, modal headers |
| Instruction / Body | `DM Sans`, `Outfit`, sans-serif | 400 | Normal | 14px / 1.55 / `-0.01em` | Execution context, progressive steps |
| Controls / CTA | `DM Sans`, `Outfit`, sans-serif | 600 | Normal | 14px / 1.25 / `-0.005em` | Pill action buttons, completion CTA |
| Telemetry / Crypto | `ui-monospace`, `SF Mono`, monospace | 500 | Normal | 12px / 1.35 / `+0.04em` | Timers, BIP-39 words, HLC indicators |

### CSS Configuration
```css
:root {
  --font-directive: 'DM Sans', -apple-system, BlinkMacSystemFont, sans-serif;
  --font-body: 'Roboto', -apple-system, BlinkMacSystemFont, sans-serif;
  --font-mono: ui-monospace, 'SF Mono', Menlo, Consolas, monospace;
  --font-serif: 'Doppio One', Georgia, 'Times New Roman', serif;

  --text-directive: 1.75rem;     /* 28px */
  --text-directive-lead: 1.125rem;/* 18px */
  --text-body: 0.9375rem;        /* 15px */
  --text-caption: 0.8125rem;     /* 13px */
  --text-mono-hud: 0.75rem;      /* 12px */
}

.display-directive-title {
  font-family: var(--font-directive);
  font-weight: 700;
  font-size: var(--text-directive);
  line-height: 1.25;
  letter-spacing: -0.03em;
  color: var(--color-text-primary);
}

.hud-mono-tag {
  font-family: var(--font-mono);
  font-weight: 500;
  font-size: var(--text-mono-hud);
  letter-spacing: 0.05em;
  text-transform: uppercase;
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
a **System Telemetry** group (Entropy Log, Trajectory), an **Active
Worldlines** readout, and Settings. The worldlines are a *readout*: no
hover state, no pointer cursor, because a tappable row with no detail
page behind it is a dead affordance.

```html
<div class="wl-float-layer">
  <button class="wl-float-btn wl-float-menu" aria-label="Open navigation">
    <svg class="wl-icon-menu wl-icon" view_box="0 0 18 18" fill="none"
         stroke="currentColor" stroke_width="1.6" stroke_linecap="round"
         aria-hidden="true"><path d="M3 5h12M3 9h12M3 13h12" /></svg>
  </button>
</div>
```

**Icons are inline SVG, never font glyphs.** A glyph's weight, spacing, and (on many Linux desktops) its colour are chosen by the font stack — which is exactly where the drawer's settings-icon colour and size drift came from. Every icon strokes with `currentColor` so it inherits the theme tokens for free, is sized by CSS only (`.wl-icon` + a per-icon modifier, never a `width`/`height` attribute), and is `aria-hidden` because the control around it already carries the real `aria-label`. The `✚` create-goal glyph is the one deliberate exception: a plus is unambiguous at 20px and never suffered the drift.

**The settings mark is a gear**, drawn as one filled path with `fill-rule="evenodd"`: the outer sub-path is the notched rim, the trailing circle sub-path is the hub hole, and evenodd knocks it out. It is therefore the one icon that is `fill: currentColor` while the others are stroked. Its geometry was measured off a reference mark (outer r 8.6 on a 24 grid, hole r 3.4 ≈ 0.40 of outer, root r 7.0, eight teeth at ±15°) after two attempts that were rendered and rejected: a stroked ring-plus-teeth version reads as a **sun**, and deeper/wider notches read as a **ship's wheel**. Rejected-by-rendering is the normal path for icon geometry here — none of these three could be judged by reading the path data.

```css
/* Transparent to input: the layer spans the width but must not eat
   clicks meant for the directive card beneath it. */
.wl-float-layer {
  position: absolute; top: 0; left: 0; right: 0; z-index: 10;
  display: flex; align-items: flex-start; justify-content: space-between;
  padding: 14px 16px 0;
  pointer-events: none;
}
.wl-float-btn {
  pointer-events: auto;
  display: inline-flex; align-items: center; justify-content: center;
  width: 34px; height: 34px; border-radius: 50%;
  border: 1px solid var(--wl-border-subtle);
  background: var(--wl-surface-elevated);
  color: var(--wl-text-primary);
  font-size: 15px; line-height: 1; cursor: pointer;
  /* The raised look: elevated surface + soft drop shadow, so it reads
     as sitting above the content rather than embedded in it. */
  box-shadow: 0 2px 8px rgba(19, 19, 18, 0.55);
  /* Must name a token that EXISTS (`--duration-instant`). This once read
     `var(--duration-fast)`, which is defined nowhere — the declaration
     was invalid, so the transition silently collapsed to 0s. */
  transition: transform var(--duration-instant) var(--ease-tactile),
              background-color var(--duration-instant) var(--ease-tactile);
}
.wl-float-btn svg { display: block; }
```

Two rules this section now encodes:

- **There is no session timer.** The `MM:SS` readout, its 1 Hz ticker, and the whole `TimerSession` state were removed 2026-09-26. The canvas shows no elapsed time at all; work is bounded by the directive, not by a clock. `elapsed_secs` survives only for sync freshness (`SYNCED · 42s ago`) in the telemetry drawer, which is not timer-shaped — do not reintroduce a timer from it.
- **`.wl-hud` survives only as a generic inline status row** used by `byok.rs`. Its former `border-bottom` + `padding-bottom` (what made it read as a full-width bar) must **not** be restored.

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
  font-family: var(--font-mono-telemetry); font-size: 11px;
  text-transform: uppercase; color: var(--wl-accent-coral);
  letter-spacing: 0.05em; margin-bottom: 14px;
}
.wl-directive-title {
  font-family: var(--font-sans-directive);
  font-size: 24px; font-weight: 700;
  line-height: 1.3; letter-spacing: -0.025em;
  color: var(--wl-text-primary); margin: 0 0 14px 0;
}
.wl-directive-instruction {
  font-size: 14px; line-height: 1.6;
  color: var(--wl-text-secondary); margin: 0 0 24px 0;
}
.wl-code {
  font-family: var(--font-mono-telemetry);
  background: var(--wl-surface-elevated);
  padding: 2px 6px; border-radius: 4px;
  color: var(--wl-text-primary); font-size: 12px;
}
.wl-progress-track {
  width: 100%; height: 4px;
  background-color: var(--wl-surface-high); border-radius: 2px; overflow: hidden;
}
.wl-progress-bar {
  height: 100%; background-color: var(--wl-cta-bg); transition: width 200ms ease;
}
```

### C. Execution & the Escape Hatch

**The canvas carries no action buttons at all** (2026-09-27). The footer
that held "Complete Directive" and "Bailout / Blocked" is gone. The
canvas is the directive; completion is `⌘+Enter` and the escape hatch is
`Escape`, and the menu is the only control on the surface. A second pair
of buttons under the card competed with the one thing the product is
for. The trade is deliberate and worth stating: discoverability now rests
on the two shortcuts, so any new surface must re-teach them.

The escape modal itself is unchanged and still required — categorising the
reason is the friction, and the friction did not move, only the trigger.

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

`.wl-btn-primary` and `.wl-btn-escape` are still the CTA and the
frictionful bail everywhere else — the compose and dormant
screens. Only the canvas footer is gone.
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
.wl-btn-primary:hover {
  background-color: var(--wl-cta-hover); transform: translateY(-1px);
}
.wl-kbd {
  font-family: var(--font-mono-telemetry); font-size: 11px;
  background: rgba(0, 0, 0, 0.12); padding: 2px 6px; border-radius: 4px;
}

/* Frictionful Escape Hatch Button */
.wl-btn-escape {
  width: 100%; height: 42px;
  background-color: transparent; color: var(--wl-text-muted);
  font-family: var(--font-sans-directive); font-size: 13px; font-weight: 500;
  border-radius: var(--wl-radius-btn); border: 1px solid var(--wl-border-subtle);
  cursor: pointer; display: flex; justify-content: center; align-items: center; gap: 8px;
  transition: background-color 120ms ease, color 120ms ease;
}
.wl-btn-escape:hover {
  background-color: var(--wl-surface-card); color: var(--wl-accent-coral);
  border-color: rgba(226, 109, 82, 0.3);
}
.wl-kbd-subtle {
  font-family: var(--font-mono-telemetry); font-size: 10px; opacity: 0.7;
}
```

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
  font-family: var(--font-serif); font-size: 28px; font-weight: 400;
  margin: 0 0 8px 0; color: var(--wl-text-primary);
}
.wl-italic-accent { font-style: italic; color: var(--wl-accent-coral); }
.wl-seed-sub { font-size: 13px; line-height: 1.5; color: var(--wl-text-muted); margin: 0; }
.wl-seed-grid { display: grid; grid-template-columns: repeat(2, 1fr); gap: 8px; }
.wl-seed-token {
  background-color: var(--wl-surface-card); border: 1px solid var(--wl-border-subtle);
  border-radius: var(--wl-radius-pill); padding: 8px 12px;
  font-family: var(--font-mono-telemetry); font-size: 13px; color: var(--wl-text-primary);
  display: flex; align-items: center; gap: 8px;
}
.wl-seed-num { color: var(--wl-text-muted); font-size: 10px; }
```

---

## 5. Implementation Rules

### Do:
- **Lock the Viewport:** Constrain the desktop UI to `420px × 747px` (`resizable: false`, `maximizable: false`). Never allow layout stretch on widescreen monitors.
- **Enforce the Single-Command Rule:** Only one active directive container may be rendered in the DOM at any given execution cycle.
- **Emphasize Primary CTA Contrast:** The execution button (`.wl-btn-primary`) must always be rendered in warm cream `#DAD5C7` to act as an unequivocal behavioral magnet.
- **Render Telemetry in Monospace:** All durations, HLC sequence stamps, and sync counts must use `var(--font-mono-telemetry)` to prevent tabular jitter. (Countdowns no longer exist — the session timer was removed 2026-09-26 — but the rule stands for whatever numeric readout comes next.)
- **Require Confirmation on Escape Hatch:** The escape hatch (now `Escape`, formerly the `.wl-btn-escape` button) must open a modal requiring the user to categorize the stall (`Blocked`, `Scope`, `Energy`) before unmounting the directive.
- **Use switches for boolean preferences.** Theme and window pin are `.wl-switch[role=switch]` with `aria-checked`, not a `<select>` and not a label-bearing button. They apply **and persist immediately** — a theme toggle that only takes effect on Save is a broken control, because you cannot evaluate a theme you are not allowed to see. Text fields still wait for the page's Save button; that two-path model is deliberate.
- **A switch persists from shell-confirmed state, never from the local draft.** `settings_save` writes every field, so persisting the draft would silently commit whatever the user had half-typed into an unrelated field.
- **Switch state colours are absolute, not theme-derived.** The switch encodes "which theme is selected", not "what is currently rendered"; flipping its tokens with `data-theme` would invert the control the instant it took effect.
- **The control panel is a readout, not a browser.** Active Worldlines rows carry no hover state and no pointer cursor. Do not make them tappable without a goal-detail page to land on.
- **An inert number must say it is inert.** `estimate_adjustment` is observed and not applied (PRD delta 33); every surface showing it repeats that. A figure that looks like a setting but changes nothing is the one thing this system will not ship.
- **Never fabricate a read-out under the mock.** Every `dx serve` response is canned. Anything the drawer renders from a command says `· MOCK` when `transport()` reports the mock, as the model catalog already does (PRD delta 148).

### Don't:
- **Never display a scrollable list of future tasks:** The home screen must never show what's coming up this afternoon or tomorrow.
- **Never use streaks or red failure states:** Missing a directive is a velocity adjustment event, never an alarm. Avoid punitive UI red `#FF0000`.
- **Never use pure white (`#FFF`) or deep black (`#000`):** Use the specified slate canvas (`#131312`) and muted text tones (`#E5E2E0` / `#CAC6BC`). This holds for switch knobs and tracks too.
- **No decorative background illustrations or icons:** Maintain tactile, terminal-level discipline. Avoid extraneous emojis and marketing illustrations. UI icons are functional inline SVG (see §4.A); emoji are not UI iconography.
- **Never put a control back in the canvas chrome for convenience.** Sync moved to Settings deliberately, and the Complete/Bailout footer was removed entirely; the canvas keeps exactly one floating control.
- **Never split a compose field into a title field plus a details field.** The user types intent in their own words and the architect names the goal; see §4.A3. Corollary: a goal title derived from a raw fragment must be clipped before it reaches `create_goal`, because the manual path seeds the first directive with that title verbatim.
- **Never accent a page title.** Page titles are one plain serif line in `--wl-text-primary`. The coral beacon belongs to the active step badge, the create-goal button, and cryptographic security badges — not to a heading, and not to a decorative word inside one.

---

## 6. Performance Budget (Wasm & Local-First Targets)
1. **Zero Idle GPU Overhead:** No infinite glowing CSS animations or heavy `backdrop-filter: blur()` layers inside the 9:16 frame.
2. **Sub-16ms Frame Time:** State updates driven by the local SQLite/CRDT outbox must re-render the directive canvas in $<16\text{ ms}$.
3. **Hardware Storage Isolation:** API keys and BIP-39 mnemonic strings must pass directly from memory to Tauri Stronghold without serializing into temporary DOM dataset attributes.

---

## 7. Quick Start Checklist
1. Bind `:root` tokens: Canvas `#131312`, Card `#20201F`, CTA `#DAD5C7`, Accent Coral `#E26D52`. (The card token is `#20201F`; an older revision of this file said `#1C1C1B` while contradicting itself two sections earlier.)
2. Restrict root viewport bounds to `420px × 747px` fixed portrait mode.
3. Wire typography: `Doppio One` for editorial headings and page titles, `DM Sans` (700 bold) for the active Stackelberg command, `ui-monospace` for telemetry.
4. Render exactly **one** primary directive card (`.wl-directive-card`) under exactly **one** floating control (the menu).
5. Wire keyboard shortcuts: `⌘+Enter` triggers completion; `Escape` triggers the frictionful bailout drawer.
6. Secondary pages centre their title over an absolutely-positioned back chevron; boolean preferences are switches that persist immediately.
