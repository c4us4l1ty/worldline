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
- Designing or coding any UI in Worldline (Stackelberg active directive canvas, daily briefing, evening audit, BIP-39 recovery screen).
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
| `--bg-cream-primary` | `#DAD5C7` | Primary execution CTA ("Complete Directive") |
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
Overlays the canvas instead of sitting in a bar above it. A full-width header strip fought the core premise — ONE directive, no chrome — and squeezed the empty state into a letterbox, so the bar was removed (2026-09-26). The menu is a floating circular control (top-left); the sync control floats opposite it (top-right). Both are raised off the surface with a soft shadow.

```html
<div class="wl-float-layer">
  <button class="wl-float-btn wl-float-menu" aria-label="Open navigation">☰</button>
  <div class="wl-float-right">
    <button class="wl-float-btn wl-float-sync" aria-label="Sync now">⇅</button>
  </div>
</div>
```

```css
/* Transparent to input: the layer spans the width but must not eat
   clicks meant for the directive card beneath it. */
.wl-float-layer {
  position: absolute; top: 0; left: 0; right: 0; z-index: 10;
  display: flex; align-items: flex-start; justify-content: space-between;
  padding: 14px 16px 0;
  pointer-events: none;
}
.wl-float-right { display: flex; align-items: center; gap: 8px; pointer-events: none; }
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
}
```

Two rules this section now encodes:

- **There is no session timer.** The `MM:SS` readout, its 1 Hz ticker, and the whole `TimerSession` state were removed 2026-09-26. The canvas shows no elapsed time at all; work is bounded by the directive, not by a clock. `elapsed_secs` survives only for sync freshness (`SYNCED · 42s ago`) in the telemetry drawer, which is not timer-shaped — do not reintroduce a timer from it.
- **`.wl-hud` survives only as a generic inline status row** used by `byok.rs`. Its former `border-bottom` + `padding-bottom` (what made it read as a full-width bar) must **not** be restored.

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

### C. Stackelberg Action Controls & Escape Hatch
Controls are anchored at the bottom of the viewport: primary execution completion paired with the deliberate friction bailout.

```html
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

```css
.wl-actions { display: flex; flex-direction: column; gap: 10px; padding-top: 16px; }

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
  font-family: var(--font-serif-briefing); font-size: 28px; font-weight: 400;
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
- **Require Confirmation on Escape Hatch:** The bailout button (`.wl-btn-escape`) must open a modal requiring the user to categorize the stall (`Blocked`, `Scope`, `Energy`) before unmounting the directive.

### Don't:
- **Never display a scrollable list of future tasks:** The home screen must never show what's coming up this afternoon or tomorrow.
- **Never use streaks or red failure states:** Missing a directive is a velocity adjustment event, never an alarm. Avoid punitive UI red `#FF0000`.
- **Never use pure white (`#FFF`) or deep black (`#000`):** Use the specified slate canvas (`#131312`) and muted text tones (`#E5E2E0` / `#CAC6BC`).
- **No decorative background illustrations or icons:** Maintain tactile, terminal-level discipline. Avoid extraneous emojis and marketing illustrations.

---

## 6. Performance Budget (Wasm & Local-First Targets)
1. **Zero Idle GPU Overhead:** No infinite glowing CSS animations or heavy `backdrop-filter: blur()` layers inside the 9:16 frame.
2. **Sub-16ms Frame Time:** State updates driven by the local SQLite/CRDT outbox must re-render the directive canvas in $<16\text{ ms}$.
3. **Hardware Storage Isolation:** API keys and BIP-39 mnemonic strings must pass directly from memory to Tauri Stronghold without serializing into temporary DOM dataset attributes.

---

## 7. Quick Start Checklist
1. Bind `:root` tokens: Canvas `#131312`, Card `#1C1C1B`, CTA `#DAD5C7`, Accent Coral `#E26D52`.
2. Restrict root viewport bounds to `420px × 747px` fixed portrait mode.
3. Wire typography: `Doppio One` for morning greetings, `DM Sans` (700 bold) for the active Stackelberg command, `ui-monospace` for telemetry and timers.
4. Render exactly **one** primary directive card (`.wl-directive-card`).
5. Wire keyboard shortcuts: `⌘+Enter` triggers completion; `Escape` triggers the frictionful bailout drawer.
