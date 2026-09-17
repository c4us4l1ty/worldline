# Worldline Frontend Architecture & UI/UX Design System Specification

---

## 1. Global View State Machine & Architecture

Worldline uses a deterministic, linear Finite State Machine (FSM). There are no parallel routing trees, nested backlogs, or persistent bottom tab bars. At any timestamp $t$, the client evaluates exactly one primary state within the fixed 9:16 terminal viewport (`420px × 747px`).

```
                    ┌─────────────────────────┐
                    │      UNINITIALIZED      │
                    └────────────┬────────────┘
                                 │ First Boot (No Seed Found)
                                 ▼
                    ┌─────────────────────────┐
                    │   BIP39_SEED_VAULT      │
                    └────────────┬────────────┘
                                 │ Seed Verified (3-Word Challenge)
                                 ▼
                    ┌─────────────────────────┐
                    │      BYOK_KEY_STORE     │
                    └────────────┬────────────┘
                                 │ Key Stored in Stronghold
                                 ▼
            ┌─────────────────────────────────────────────────┐
            │               OPERATIONAL RUNTIME               │
            │                                                 │
            │    ┌──────────────────┐                         │
            │    │ MORNING_BRIEFING │ (Daily Trigger: 05:00)  │
            │    └────────┬─────────┘                         │
            │             │ Briefing Confirmed                │
            │             ▼                                   │
            │    ┌──────────────────┐    Escape / Bailout     │
            │    │ DIRECTIVE_ACTIVE │ ◄──────────────────┐    │
            │    └────────┬─────────┘                    │    │
            │             │               ┌──────────────┴──┐ │
            │             ├──────────────►│ ESCAPE_HATCH_   │ │
            │             │ (Esc Pressed) │ MODAL           │ │
            │             │ Complete      └─────────────────┘ │
            │             ▼ (Cmd+Enter)                       │
            │    ┌──────────────────┐                         │
            │    │  EVENING_AUDIT   │ (Daily Trigger: 21:00)  │
            │    └────────┬─────────┘                         │
            │             │ Velocity Recalibrated             │
            │             ▼                                   │
            │    ┌──────────────────┐                         │
            │    │     DORMANT      │ (Until Next Milestone)  │
            │    └──────────────────┘                         │
            └─────────────────────────────────────────────────┘
                                 │
                   Global Hotkey │ Cmd + ,
                                 ▼
                    ┌─────────────────────────┐
                    │    SYSTEM_TELEMETRY     │
                    │         DRAWER          │
                    └─────────────────────────┘
```

### Global Input Bindings

| Keybinding | Scope | Action |
|---|---|---|
| `⌥ Space` (`Alt+Space`) | Global OS | Summon / Dismiss 9:16 Ambient Floating Window |
| `⌘ ↵` (`Ctrl+Enter`) | `DIRECTIVE_ACTIVE` | Commit directive completion; advance progressive phase |
| `Esc` | `DIRECTIVE_ACTIVE` | Mount the Frictionful Escape Hatch modal |
| `⌘ ,` (`Ctrl+,`) | Operational Runtime | Open System Telemetry & Stronghold Key Drawer |
| `1` – `3` | `ESCAPE_HATCH_MODAL` | Select stall reason (`Dependency`, `Scope`, `Energy`) |
| `Tab` / `Shift+Tab` | All | Strict programmatic visual focus ring traversal |

---

## 2. Screen-by-Screen UI/UX Specifications

```
+-------------------------------------------------------------+
| Desktop Form Factor: 420px x 747px (Strict 9:16)            |
| resizable: false | maximizable: false | fullscreen: false   |
+-------------------------------------------------------------+
```

---

### Screen 1: Zero-Knowledge BIP-39 Sovereign Onboarding

```
+-----------------------------------------------------------+ [420px]
|  [HLC: 0.0.0]                                  NET: LOCAL |
|                                                           |
|  Sovereign                                                |
|  *trajectory*                                             |
|                                                           |
|  Write down these 12 words in sequential order.           |
|  Worldline operates on zero-knowledge cryptography:       |
|  there are no accounts, recovery emails, or reset links.  |
|                                                           |
|  +--------------------+   +--------------------+          |
|  | 01  beacon         |   | 02  orbit          |          |
|  +--------------------+   +--------------------+          |
|  | 03  silence        |   | 04  dynamic        |          |
|  +--------------------+   +--------------------+          |
|  | 05  marble         |   | 06  drift          |          |
|  +--------------------+   +--------------------+          |
|  | 07  lattice        |   | 08  kinetic        |          |
|  +--------------------+   +--------------------+          |
|  | 09  harbor         |   | 10  canyon         |          |
|  +--------------------+   +--------------------+          |
|  | 11  velvet         |   | 12  anchor         |          |
|  +--------------------+   +--------------------+          |
|                                                           |
|  [!] Verified via local hardware-backed entropy (RNG).    |
|                                                           |
|  +-----------------------------------------------------+  |
|  | [ Warm Cream CTA ] Verify Seed Backup          ->   |  |
|  +-----------------------------------------------------+  |
+-----------------------------------------------------------+ [747px]
```

#### Visual Specification & Tokens
*   **Canvas Base:** `#131312` (Matte Graphite)
*   **Title:** `Doppio One` 38px / 1.15, text `#E5E2E0`. The accent word "*trajectory*" renders in `Georgia` Italic `#E26D52` (Coral).
*   **Seed Token Box:** `#20201F` background, border `1px solid rgba(229, 226, 224, 0.08)`, border-radius `8px`.
*   **Word Index:** `SF Mono` 10px `#949087`.
*   **Word Content:** `SF Mono` 13px 500 weight `#E5E2E0`.
*   **Primary CTA:** `#DAD5C7` (Warm Cream), text `#1D1C13` (Charcoal). Hover: `#ECE6DA`. Height: `52px`.

#### Interaction & Transition Behavior
1.  **Entropy Generation:** Generated via Rust `getrandom` OS-level entropy, parsed to a 12-word phrase via the BIP-39 English wordlist.
2.  **Challenge Transition:** Clicking **Verify Seed Backup** slides in a secondary card requiring validation of 3 randomized indices (e.g., words #03, #07, #11).
3.  **Security Rule:** Seed strings are zeroized in RAM immediately after validation and derived to an Ed25519 authentication key and a ChaCha20-Poly1305 symmetric encryption root.

---

### Screen 2: BYOK Engine Initialization & Secret Storage

```
+-----------------------------------------------------------+ [420px]
|  IDENTITY: 0x9f4a...b3e1                       VAULT: OK  |
|                                                           |
|  Execution                                                |
|  *intelligence*                                           |
|                                                           |
|  Worldline delegates planning to your own frontier keys.  |
|  Credentials pass directly into OS Stronghold isolation.  |
|                                                           |
|  ACTIVE PROVIDER                                          |
|  +-----------------------------------------------------+  |
|  | [x] Anthropic      [ ] OpenAI      [ ] OpenRouter   |  |
|  +-----------------------------------------------------+  |
|                                                           |
|  ANTHROPIC API KEY                                        |
|  +-----------------------------------------------------+  |
|  | sk-ant-api03-...................................... |  |
|  +-----------------------------------------------------+  |
|  [i] Injected directly into tauri-plugin-stronghold       |
|                                                           |
|  MODEL DISPATCH ALLOCATION                                |
|  - Tier 1 (Architect):   Claude 3.5 Sonnet                |
|  - Tier 2 (Dispatcher):  Claude 3.5 Haiku                 |
|  - Tier 3 (Local Core):  Rust Native Deterministic Engine  |
|                                                           |
|  +-----------------------------------------------------+  |
|  | [ Warm Cream CTA ] Initialize Sovereign Workspace   |  |
|  +-----------------------------------------------------+  |
+-----------------------------------------------------------+ [747px]
```

#### Visual Specification & Tokens
*   **Provider Selector:** `#20201F` pill track, `#2A2A29` active segment with `1px solid rgba(229, 226, 224, 0.12)`.
*   **Secure Input Field:** `#1C1C1B` background, border `1px solid rgba(229, 226, 224, 0.12)`, font `SF Mono` 12px `#E5E2E0`. Active focus ring: `1px solid #E26D52`.
*   **Model Routing Matrix:** `#20201F` card container, border `1px solid rgba(229, 226, 224, 0.08)`, text `#949087`, labels `#E5E2E0`.

#### Security Execution Profile
*   Keys pass across the Tauri IPC bridge using binary byte vectors (`Vec<u8>`).
*   Keys are persisted via `tauri-plugin-stronghold` using Argon2id key-derivation and are never written to unencrypted SQLite rows or logged in devtools.

---

### Screen 3: Morning Tactical Briefing

```
+-----------------------------------------------------------+ [420px]
|  05:00 UTC                                 HLCSync: ACTIVE |
|                                                           |
|  Good morning.                                            |
|  Today's *trajectory*                                     |
|                                                           |
|  MILESTONE 02/05                                          |
|  +-----------------------------------------------------+  |
|  | CRDT Sync Outbox & WAL SQLite Engine                |  |
|  | Trajectory: 1.4 tasks/day · Target finish: May 12   |  |
|  +-----------------------------------------------------+  |
|                                                           |
|  DISPATCHED DIRECTIVES FOR TODAY                          |
|                                                           |
|  [01] Draft SQLite schema migrations for CRDT outbox      |
|       Est: 25 mins · Deep Execution                       |
|                                                           |
|  [02] Implement monotonic HLC counter in Rust core        |
|       Est: 45 mins · Sequential Dependency                |
|                                                           |
|  [03] Axum relay authentication handshake                 |
|       Est: 30 mins · Network Layer                        |
|                                                           |
|  "Eliminate decision overhead. Follow the vector."        |
|                                                           |
|  +-----------------------------------------------------+  |
|  | [ Warm Cream CTA ] Commence Directive 01       Cmd+Enter |
|  +-----------------------------------------------------+  |
+-----------------------------------------------------------+ [747px]
```

#### Visual Specification & Tokens
*   **Briefing Title:** `Doppio One` 38px `#E5E2E0` with italic focal accent.
*   **Directive Queue Preview:** Directives are styled with `#949087` (muted slate) for remaining items and `#E5E2E0` for Directive 01.
*   **Stackelberg Rule:** This is the *only* screen where upcoming daily directives appear, providing context before single-task focus begins.

---

### Screen 4: The Stackelberg Single-Directive Canvas (Core Operational Hub)

```
+-----------------------------------------------------------+ [420px]
|  [M-02/05] CRDT OUTBOX                   (*) 24:58  [SYNC]|
|-----------------------------------------------------------|
|                                                           |
|  PHASE 1 OF 2 · ACTIVATION STEP                           |
|                                                           |
|  +-----------------------------------------------------+  |
|  |                                                     |  |
|  |  Draft the initial schema migrations                |  |
|  |  for SQLite persistence                             |  |
|  |                                                     |  |
|  |  Define tables for `identity_config` and            |  |
|  |  `crdt_outbox`. Keep primary keys as                |  |
|  |  deterministic UUIDv4. Do not wire API sync yet.   |  |
|  |                                                     |  |
|  |  ========================                           |  |
|  |  [PROGRESS TRACK: 50%]                              |  |
|  +-----------------------------------------------------+  |
|                                                           |
|  PROGRESSIVE ACTIVATION                                   |
|  [*] Phase 1: Function signature definition     (05 mins) |
|  [ ] Phase 2: Complete SQLite statement parser (20 mins) |
|                                                           |
|-----------------------------------------------------------|
|  +-----------------------------------------------------+  |
|  | [ Warm Cream CTA ] Complete Directive           Cmd+Enter|
|  +-----------------------------------------------------+  |
|  +-----------------------------------------------------+  |
|  | [ Muted Outline ] Bailout / Blocked             Esc    |  |
|  +-----------------------------------------------------+  |
+-----------------------------------------------------------+ [747px]
```

#### Visual Specification & Tokens
```css
/* Core Viewport Bounds */
.wl-viewport {
  width: 420px;
  height: 747px;
  background-color: #131312;
  display: flex;
  flex-direction: column;
  justify-content: space-between;
  padding: 20px 24px;
  box-sizing: border-box;
  overflow: hidden;
  user-select: none;
}

/* Ambient HUD */
.wl-hud-status {
  display: flex;
  justify-content: space-between;
  align-items: center;
  font-family: ui-monospace, 'SF Mono', monospace;
  font-size: 11px;
  letter-spacing: 0.04em;
  color: #949087;
  padding-bottom: 14px;
  border-bottom: 1px solid rgba(229, 226, 224, 0.08);
}
.wl-timer-badge {
  color: #E26D52;
  background: #20201F;
  padding: 3px 8px;
  border-radius: 9999px;
  font-weight: 600;
}

/* Stackelberg Directive Card */
.wl-card-directive {
  background-color: #20201F;
  border: 1px solid rgba(229, 226, 224, 0.12);
  border-radius: 16px;
  padding: 24px;
  box-shadow: 0 8px 32px rgba(0, 0, 0, 0.45);
}
.wl-phase-indicator {
  font-family: ui-monospace, 'SF Mono', monospace;
  font-size: 11px;
  text-transform: uppercase;
  color: #E26D52;
  letter-spacing: 0.06em;
  margin-bottom: 12px;
}
.wl-directive-heading {
  font-family: 'DM Sans', sans-serif;
  font-size: 24px;
  font-weight: 700;
  line-height: 1.3;
  letter-spacing: -0.03em;
  color: #E5E2E0;
  margin-bottom: 12px;
}
.wl-directive-subtext {
  font-family: 'DM Sans', sans-serif;
  font-size: 14px;
  line-height: 1.55;
  color: #CAC6BC;
}
```

---

### Screen 5: The Frictionful Escape Hatch (Anti-Stall Modal)

```
+-----------------------------------------------------------+ [420px]
|  [DIMMED CANVAS BACKDROP: rgba(19,19,18,0.85)]            |
|                                                           |
|  +-----------------------------------------------------+  |
|  | DIAGNOSTIC: ESCAPE HATCH                            |  |
|  |                                                     |  |
|  | Select stall cause. Worldline will recalibrate      |  |
|  | your velocity with zero punitive alarms.            |  |
|  |                                                     |  |
|  | [1] DEPENDENCY BLOCKED                              |  |
|  |     Waiting for 3rd party API, merge, or response   |  |
|  |                                                     |  |
|  | [2] SCOPE MISCALCULATION                            |  |
|  |     Directive exceeds allotted time boundary (>2x)  |  |
|  |                                                     |  |
|  | [3] COGNITIVE / ENERGY DEPLETION                    |  |
|  |     Focus ceiling reached; request downscaled task  |  |
|  |                                                     |  |
|  | OPTIONAL CONTEXT (MAX 140 CHARACTERS)               |  |
|  | +-------------------------------------------------+ |  |
|  | SQLite table locking during test migrations       | |  |
|  | +-------------------------------------------------+ |  |
|  |                                                     |  |
|  | +-------------------------------------------------+ |  |
|  | | [ Coral Alert CTA ] Confirm Bailout & Downsize  | |  |
|  | +-------------------------------------------------+ |  |
|  |                                                     |  |
|  | [ Esc ] Resume Execution Directive                  |  |
|  +-----------------------------------------------------+  |
|                                                           |
+-----------------------------------------------------------+ [747px]
```

#### Anti-Stall Mechanics
*   **The Problem:** Unrestricted skip buttons encourage procrastination; rigid lockouts lead to abandonment.
*   **The Friction Mechanism:** Users must select an explicit categorization key (`1`, `2`, or `3`).
*   **System Action:**
    *   `Dependency Blocked`: Flags directive as `blocked`, shifts SQLite record to parking state, and loads an independent parallel milestone directive.
    *   `Scope Miscalculation`: Calls Tier 2 AI dispatcher to divide the directive into two smaller tasks without updating the daily completion quota.
    *   `Cognitive Depletion`: Downsizes the active command to a low-effort operational task (e.g., "Review schema formatting") and triggers a 10-minute rest timer.

---

### Screen 6: Evening Velocity Audit (Zero-Guilt Recalibration)

```
+-----------------------------------------------------------+ [420px]
|  21:00 UTC                                      HLC: DONE  |
|                                                           |
|  Evening                                                  |
|  *recalibration*                                          |
|                                                           |
|  Today's Vector Audit                                     |
|  3 Directives Dispatched · 2 Completed · 1 Bailout        |
|                                                           |
|  +-----------------------------------------------------+  |
|  | [v] Schema migrations for CRDT outbox     COMPLETED |  |
|  | [v] Monotonic HLC counter implementation  COMPLETED |  |
|  | [!] Axum relay authentication handshake   RE-ROUTED |  |
|  +-----------------------------------------------------+  |
|                                                           |
|  TRAJECTORY COMPUTATION                                   |
|  Remaining Milestone Scope:   8 Directives                |
|  Days to Milestone Horizon:   6 Days                      |
|                                                           |
|  Required Velocity:           1.33 Directives / Day       |
|  Current Rolling Average:     1.41 Directives / Day       |
|                                                           |
|  Trajectory Status:           ON SCHEDULE (+0.08 delta)   |
|                                                           |
|  "No debt carried forward. Plan recalculated cleanly."    |
|                                                           |
|  +-----------------------------------------------------+  |
|  | [ Warm Cream CTA ] Commit Vector & Rest                 |  |
|  +-----------------------------------------------------+  |
+-----------------------------------------------------------+ [747px]
```

#### The Zero-Guilt GPS Velocity Formula
Worldline avoids streaks, broken hearts, and punitive red styling. Velocity is computed using a navigational recalibration model:

$$V_{\text{target}} = \frac{\Delta \text{Remaining Milestones}}{\Delta \text{Remaining Days}}$$

*   Skipped or abandoned tasks are treated like an off-ramp navigation recalculation: the model reassesses remaining scope against the milestone horizon, adjusting tomorrow's directives without visual guilt or accumulated backlogs.

---

### Screen 7: Ambient Settings & System Telemetry Drawer

```
+-----------------------------------------------------------+ [420px]
|  SYSTEM TELEMETRY & VAULT                   [x] CLOSE     |
|-----------------------------------------------------------|
|                                                           |
|  HARDWARE KEYCHAIN (STRONGHOLD)                           |
|  Status:               ENCLAVE LOCKED                     |
|  Active Key Derivation: Argon2id · 128-bit Salt           |
|  Auth Public Key:      0x9f4a28c418e2eb0a                 |
|                                                           |
|  CRDT SYNCHRONIZATION RUNTIME                             |
|  Hybrid Logical Clock: 1711928400.0001_node01             |
|  Local Outbox Queue:   0 pending operations               |
|  Relay Network State:  CONNECTED (e2ee-blind-relay)       |
|  SQLite WAL Size:      248 KB                             |
|                                                           |
|  DESKTOP VIEWPORT RUNTIME                                 |
|  Window Geometry:      420px x 747px (9:16 Fixed)         |
|  Pin to Top Level:     [x] ALWAYS ON TOP (Float)          |
|  Global Summon Key:    [ Opt + Space ]                    |
|                                                           |
|  +-----------------------------------------------------+  |
|  | [ Subtle Border ] Force CRDT Peer Re-Sync           |  |
|  +-----------------------------------------------------+  |
|  +-----------------------------------------------------+  |
|  | [ Danger Coral  ] Purge Local Enclave Keys          |  |
|  +-----------------------------------------------------+  |
+-----------------------------------------------------------+ [747px]
```

---

## 3. Interaction Design & Sensory System

### 1. Motion Architecture & Timings

```css
:root {
  --ease-tactile: cubic-bezier(0.16, 1, 0.3, 1); /* Quick snap, smooth settle */
  --duration-instant: 120ms;
  --duration-settle: 200ms;
  --duration-modal: 260ms;
}

/* Micro-Interaction Tokens */
.interactive-action {
  transition: transform var(--duration-instant) var(--ease-tactile),
              background-color var(--duration-instant) var(--ease-tactile),
              box-shadow var(--duration-instant) var(--ease-tactile);
}

.interactive-action:active {
  transform: scale(0.985) translateY(1px);
}
```

*   **Directive Advance Transition:** When `⌘ ↵` is triggered:
    1. The active card drops 2px, scales to 99%, and fades to opacity `0.0` over `140ms`.
    2. The progress track bar fills to 100% in a `#E26D52` coral pulse.
    3. The next progressive directive enters from `translateY(8px)` to `0px` over `200ms` using `--ease-tactile`.
*   **Escape Hatch Entry:** Modal overlay opacity shifts from `0.0` to `1.0` in `160ms`. The diagnostic container translates upward from `translateY(16px)` to `0px`.
*   **GPU Frame Budget:** Zero continuous CSS infinite loops. The timer component triggers isolated 1-second text-node repaints using `font-variant-numeric: tabular-nums` to eliminate layout recalculations.

### 2. Tactile Color Mapping

| Token Semantic Name | Dark Mode (Default) | Light Canvas (Parchment) | Minimum Contrast Ratio |
|---|---|---|---|
| `--wl-bg-canvas` | `#131312` | `#F4F0E8` | 14.2:1 against text-primary |
| `--wl-surface-card` | `#20201F` | `#ECE6DA` | 11.8:1 against text-primary |
| `--wl-surface-elevated`| `#2A2A29` | `#E0DAD0` | 8.5:1 against text-primary |
| `--wl-cta-primary` | `#DAD5C7` | `#1D1C13` | 12.1:1 against cta-label |
| `--wl-accent-coral` | `#E26D52` | `#D8583B` | 4.8:1 against canvas (WCAG AA Large) |
| `--wl-text-primary` | `#E5E2E0` | `#1D1C13` | Meets WCAG AAA |
| `--wl-text-secondary` | `#CAC6BC` | `#68645C` | Meets WCAG AA |
| `--wl-text-muted` | `#949087` | `#827E75` | 4.6:1 (Subtext/telemetry) |

---

## 4. Frontend Component Hierarchy & State (Rust/WASM)

The client is built using Rust-native WASM UI frameworks (Leptos or Dioxus) inside the Tauri v2 desktop runtime.

```
<AppViewport (420x747 Container)>
│
├── <StatusBarHUD>
│     ├── <MilestoneBreadcrumb value={active_milestone} />
│     ├── <CountdownTimer seconds={time_remaining} />
│     └── <SyncIndicator hlc_state={sync_stream} />
│
├── <MainCanvasRouter state={runtime_state}>
│     ├── [MATCH: Uninitialized]  => <SeedVaultGenerator />
│     ├── [MATCH: ByokSetup]       => <StrongholdKeyConfig />
│     ├── [MATCH: MorningBrief]    => <MorningTrajectory />
│     ├── [MATCH: DirectiveActive] => <StackelbergDirectiveCard>
│     │                                  ├── <PhaseProgressionTracker />
│     │                                  ├── <DirectiveCommand />
│     │                                  ├── <ExecutionContext />
│     │                                  └── <ProgressTrackBar />
│     ├── [MATCH: EveningAudit]    => <EveningVelocityAudit />
│     └── [MODAL: EscapeActive]    => <FrictionfulEscapeDrawer />
│
└── <ActionDock>
      ├── <PrimaryExecutionTrigger on_commit={handle_directive_complete} />
      └── <EscapeHatchTrigger on_bailout={toggle_escape_modal} />
```

### Core Reactive State Model (Rust / Leptos Example)

```rust
use leptos::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DirectiveState {
    Queued,
    Active { started_at: u64, phase: u8 },
    Completed { completed_at: u64 },
    Blocked { reason: StallReason, notes: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum StallReason {
    Dependency,
    ScopeMiscalculation,
    CognitiveDepletion,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Directive {
    pub id: String,
    pub milestone_id: String,
    pub title: String,
    pub execution_context: String,
    pub estimated_minutes: u32,
    pub progressive_steps: u8,
    pub current_step: u8,
    pub state: DirectiveState,
    pub hlc_timestamp: String,
}

#[component]
pub fn StackelbergDirectiveCard(directive: ReadSignal<Directive>) -> impl IntoView {
    view! {
        <div class="wl-card-directive">
            <div class="wl-phase-indicator">
                {move || format!("PHASE {} OF {} · ACTIVATION STEP", 
                    directive.get().current_step, 
                    directive.get().progressive_steps
                )}
            </div>
            <h1 class="wl-directive-heading">
                {move || directive.get().title}
            </h1>
            <p class="wl-directive-subtext">
                {move || directive.get().execution_context}
            </p>
            <div class="wl-progress-track">
                <div 
                    class="wl-progress-bar"
                    style=move || format!("width: {}%;", 
                        (directive.get().current_step as f32 / directive.get().progressive_steps as f32) * 100.0
                    )
                />
            </div>
        </div>
    }
}
```

---

## 5. Persistence, Stronghold & CRDT Outbox Flow

Every user action immediately commits to the embedded SQLite database using Write-Ahead Logging (WAL) and creates an encrypted outbox record.

```
[ User Presses Cmd+Enter ]
       │
       ▼
[ Local SQLite Transaction (WAL) ]
       │
       ├── 1. UPDATE directives SET state = 'completed', hlc = :hlc WHERE id = :id;
       ├── 2. Increment local Hybrid Logical Clock (HLC).
       └── 3. INSERT INTO crdt_outbox (operation_id, hlc, table, payload);
               │
               ▼
[ Encrypt Payload Client-Side ]
       │  ChaCha20-Poly1305 symmetric encryption
       │  Derived from local BIP-39 Vault Key
       ▼
[ Blind Relayed to Axum Server ]
       │  Raw, encrypted blob + Ed25519 signature
       │  (Zero-Knowledge: Server sees no text or IDs)
       ▼
[ Convergence on Paired Mobile/Desktop ]
       └── Merged deterministically via Last-Write-Wins (LWW) HLC rules.
```

---

## 6. Edge Cases & Resilience Protocols

| Edge Case | Failure Mode | Mitigation Protocol |
|---|---|---|
| **Tier 1/2 API Offline** | Network drops or BYOK rate-limited during Morning Briefing. | System fails over to **Tier 3 Local Heuristic Engine**. Next deterministic SQLite directive deploys without waiting for LLM summarization. |
| **Window Resizing Attempt** | User attempts tiling manager expand or OS full-screen. | Tauri `tauri.conf.json` enforces `resizable: false`, `maximizable: false`, `fullscreen: false`. CSS uses fixed boundaries: `width: 420px; height: 747px; max-width: 420px;`. |
| **CRDT Causal Skew** | Offline device commits directive with older physical clock time. | Hybrid Logical Clock (HLC) coordinates monotonic order via $(l.j, c.j)$ tuple comparison, ensuring clock skew cannot overwrite newer states. |
| **Process Crash Mid-Directive** | OS kill or sudden system power failure. | SQLite WAL mode guarantees transactional atomicity. On relaunch, state restores directly to the exact active progressive phase. |
| **Memory Extraction Attack** | Malicious OS process scans RAM for BYOK secrets. | Keys are stored in `tauri-plugin-stronghold` and loaded into protected memory vectors only during active API call serialization. |

---

## 7. Implementation Verification & Polish Matrix

To ensure the client strictly adheres to the Worldline design specification, verify implementation against this matrix before shipping:

- [ ] **Viewport Constraints:** Canvas is constrained to `420px × 747px` with no horizontal or vertical window scrollbars.
- [ ] **Color Discipline:** No instances of pure `#000000` or `#FFFFFF` exist in the CSS stylesheet.
- [ ] **Single Directive Isolation:** The DOM tree contains exactly **one** primary interactive directive at any execution phase.
- [ ] **Keybinding Completeness:** `⌘+Enter` completes directives; `Esc` mounts the escape hatch modal; `⌥+Space` summons/dismisses the window globally.
- [ ] **Timer Stability:** Timers render using `var(--font-mono)` with `font-variant-numeric: tabular-nums` to prevent layout jitter.
- [ ] **Anti-Guilt UX:** Skipping a directive prompts the categorization flow; it never displays red failure warnings, alarms, or streak counters.
- [ ] **Zero-Knowledge Core:** Onboarding generates a standard 12-word BIP-39 mnemonic phrase and requires confirmation of 3 random words before initialization.
