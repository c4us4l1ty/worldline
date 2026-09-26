//! Worldline UI (Dioxus) — the Stackelberg single-directive canvas.
//!
//! Enforces the worldline skill invariants:
//! * Exactly ONE directive rendered at any moment (skill §5 "Do").
//! * 9:16 fixed portrait layout (420×747 desktop, fluid mobile).
//! * ⌘+Enter completes; Escape opens the frictionful bailout modal.
//! * Zero future lists, zero streaks, zero alarm red.

use dioxus::{core::Task, prelude::*};
use serde::{de::DeserializeOwned, Serialize};

use crate::theme::Theme;

pub fn main() {
    launch(App);
}

/// Invoke a Tauri shell command via the JS shim (native) or the mock
/// harness (browser dev). The shim resolves `window.wlInvoke`, which
/// Tauri provides through the injected `invoke-shim.js` asset.
pub async fn invoke<T: DeserializeOwned + Default>(
    cmd: &str,
    args: impl Serialize,
) -> Result<T, String> {
    let args_json = serde_json::to_string(&args).map_err(|e| e.to_string())?;
    let raw = js_invoke(cmd.to_string(), args_json).await?;
    if let Some(err) = raw.strip_prefix("__WL_ERR__:") {
        return Err(err.to_string());
    }
    serde_json::from_str(&raw).map_err(|e| e.to_string())
}

async fn js_invoke(cmd: String, args_json: String) -> Result<String, String> {
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen(inline_js = r#"
    export function invoke_raw(cmd, args_json) {
      try {
        const args = args_json && args_json !== "null" ? JSON.parse(args_json) : {};
        const p = window.wlInvoke(cmd, args);
        if (p && typeof p.then === "function") {
          return p.then(
            (v) => JSON.stringify({ ok: v === undefined ? null : v }),
            (e) => JSON.stringify({ err: String(e) })
          );
        }
        return Promise.resolve(JSON.stringify({ ok: p }));
      } catch (e) {
        return Promise.resolve(JSON.stringify({ err: String(e) }));
      }
    }
  "#)]
    extern "C" {
        fn invoke_raw(cmd: String, args_json: String) -> js_sys::Promise;
    }
    let js = wasm_bindgen_futures::JsFuture::from(invoke_raw(cmd, args_json))
        .await
        .map_err(|e| format!("{e:?}"))?;
    let text = js.as_string().unwrap_or_default();
    #[derive(serde::Deserialize)]
    struct Wrapped {
        ok: Option<serde_json::Value>,
        err: Option<String>,
    }
    let wrapped: Wrapped = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    match (wrapped.ok, wrapped.err) {
        (_, Some(e)) => Ok(format!("__WL_ERR__:{e}")),
        (Some(v), _) => Ok(v.to_string()),
        (None, None) => Ok("null".to_string()),
    }
}

// ---------------------------------------------------------------------------
// Shared UI DTOs (mirror shell view structs)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DirectiveView {
    pub directive_id: String,
    pub milestone_id: String,
    pub title: String,
    pub instruction: Option<String>,
    pub phase: Option<(i64, i64)>,
    pub estimated_minutes: i64,
    pub state: String,
    pub milestone_title: Option<String>,
}

impl<'de> serde::Deserialize<'de> for DirectiveView {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct Raw {
            directive_id: String,
            milestone_id: String,
            title: String,
            instruction: Option<String>,
            phase: Option<(i64, i64)>,
            estimated_minutes: i64,
            state: String,
            milestone_title: Option<String>,
        }
        let r = Raw::deserialize(d)?;
        Ok(DirectiveView {
            directive_id: r.directive_id,
            milestone_id: r.milestone_id,
            title: r.title,
            instruction: r.instruction,
            phase: r.phase,
            estimated_minutes: r.estimated_minutes,
            state: r.state,
            milestone_title: r.milestone_title,
        })
    }
}

#[derive(Clone, Debug, Default, serde::Deserialize, PartialEq)]
pub struct IdentityStatus {
    pub has: bool,
    pub unlocked: bool,
    #[serde(default)]
    pub vault_has_mnemonic: bool,
}

#[derive(Clone, Debug, Default, serde::Deserialize, PartialEq)]
pub struct VelocityView {
    pub milestones_remaining: i64,
    pub days_remaining: i64,
    pub target_per_day: f64,
    pub completion_ratio: f64,
    pub estimate_adjustment: f64,
}

/// B-005: shell returns what the dispatcher AUTHORED and what it
/// actually PERSISTED — the UI must not assume the two are equal
/// (no key / offline / validation failure yield zero persisted).
#[derive(Clone, Debug, Default, serde::Deserialize, PartialEq)]
pub struct BriefingView {
    pub titles: Vec<String>,
    #[serde(default)]
    pub created_ids: Vec<String>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize, PartialEq)]
pub struct AppSettingsView {
    pub theme: String,
    pub hotkey: String,
    pub always_on_top: bool,
    pub ai_provider: Option<String>,
    pub tier1_model: Option<String>,
    pub tier2_model: Option<String>,
    pub relay_url: Option<String>,
}

impl Default for AppSettingsView {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            hotkey: "alt+space".into(),
            always_on_top: false,
            ai_provider: None,
            tier1_model: None,
            tier2_model: None,
            relay_url: None,
        }
    }
}

// ---------------------------------------------------------------------------
// App state & routing (screen enum — no URL routing needed for the
// single-window terminal)
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
pub enum Screen {
    Boot,
    /// Fail-closed boot: the shell answered with an error (broken or
    /// unreadable vault/config). Renders wipe/restore guidance instead
    /// of pretending the install is fresh (MVP-5).
    BootError {
        detail: String,
    },
    SeedVault {
        phrase: Vec<String>,
        verify_indices: Vec<usize>,
        restore: bool,
        /// True when this install already has an identity (locked-boot
        /// restore). The restore screen must not offer "create new",
        /// which can never succeed then (B-004).
        has_identity: bool,
    },
    ByokSetup,
    Canvas,
    GoalCreate,
    EveningCheckIn,
    Dormant,
    MorningBrief,
    Settings,
}

/// Note: the session timer was removed from the canvas (2026-09-26, user
/// directive). With it went `TimerSession`, `timer_session`, `fmt_mmss`
/// and the 1 Hz `start_timer` ticker — they existed only to drive the
/// `MM:SS` readout, so all of it was dead weight rather than shared
/// infrastructure. `elapsed_secs` SURVIVES because `sync_label` uses it
/// for sync freshness ("SYNCED · 42s ago"), and that is not timer-shaped.
pub fn active_directive(directive: Option<DirectiveView>) -> Option<DirectiveView> {
    directive.filter(|d| d.state == "active" && !d.directive_id.is_empty())
}

/// Milliseconds elapsed between two epoch-ms stamps, clamped at zero
/// (a wall-clock step backwards must never render as a negative age).
pub fn elapsed_secs(started_ms: f64, now_ms: f64) -> u64 {
    ((now_ms - started_ms).max(0.0) / 1000.0) as u64
}

#[derive(Clone, Copy)]
pub struct AppCtx {
    pub screen: Signal<Screen>,
    pub directive: Signal<Option<DirectiveView>>,
    pub velocity: Signal<VelocityView>,
    pub settings: Signal<AppSettingsView>,
    pub toast: Signal<Option<String>>,
    pub directive_busy: Signal<bool>,
    pub toast_task: Signal<Option<Task>>,
    pub escape_open: Signal<bool>,
    pub telemetry_open: Signal<bool>,
    /// MVP-1 hamburger nav drawer (left slide; GoalCreate/Settings).
    /// Separate from the Ctrl+, telemetry drawer: the drawer is
    /// discovery navigation, telemetry is system inspection.
    pub nav_open: Signal<bool>,
    pub sync_status: Signal<String>,
    /// Epoch-ms of the last successful sync cycle (MVP-4). `None` until
    /// one succeeds, so the HUD can say "never synced" instead of
    /// implying a freshness it never had.
    pub last_sync_ms: Signal<Option<f64>>,
    /// Shared identity state (MVP-5): set at boot, refreshed after
    /// unlock/restore/generate in Settings. The drawer, canvas and
    /// settings all read the same signal.
    pub identity_status: Signal<IdentityStatus>,
}

fn App() -> Element {
    use_context_provider(|| AppCtx {
        screen: Signal::new(Screen::Boot),
        directive: Signal::new(None),
        velocity: Signal::new(VelocityView::default()),
        settings: Signal::new(AppSettingsView::default()),
        toast: Signal::new(None),
        directive_busy: Signal::new(false),
        toast_task: Signal::new(None),
        escape_open: Signal::new(false),
        telemetry_open: Signal::new(false),
        nav_open: Signal::new(false),
        sync_status: Signal::new("LOCAL".to_string()),
        last_sync_ms: Signal::new(None),
        identity_status: Signal::new(IdentityStatus::default()),
    });

    let ctx = use_context::<AppCtx>();
    let _theme = use_context_provider(Theme::new);

    // Boot: load settings/theme and route straight to Home (MVP-5).
    // No onboarding screens at boot: identity work (unlock, restore,
    // generate, 12-word phrase) lives in Settings. The ONLY boot
    // failure that blocks is the shell refusing to answer — a silent
    // default would disguise a broken vault as a fresh install.
    let mut booted = use_signal(|| false);
    use_effect(move || {
        if *booted.read() {
            return;
        }
        booted.set(true);
        let ctx2 = ctx;
        spawn(async move {
            let mut screen = ctx2.screen;
            let mut settings_sig = ctx2.settings;
            let settings: AppSettingsView = invoke("settings_get", ()).await.unwrap_or_default();
            crate::theme::apply_data_theme(if settings.theme == "light" {
                "light"
            } else {
                "dark"
            });
            *settings_sig.write() = settings;
            match invoke::<IdentityStatus>("identity_status", ()).await {
                Ok(status) => {
                    let mut st = ctx2.identity_status;
                    st.set(status);
                    *screen.write() = Screen::Canvas;
                }
                Err(e) => *screen.write() = Screen::BootError { detail: e },
            }
        });
    });

    let _screen = ctx.screen.read().clone();

    rsx! {
        div {
            class: "wl-root",
            tabindex: "0",
            onkeydown: move |e: Event<KeyboardData>| {
                // Global drawer toggle: Ctrl+, (web equivalent of spec ⌘,).
                // Escape dismisses the drawer when open.
                if e.key() == Key::Character(",".to_string()) && e.modifiers().ctrl() {
                    let open = *ctx.telemetry_open.read();
                    { let mut s = ctx.telemetry_open; *s.write() = !open; }
                } else if e.key() == Key::Escape && (*ctx.telemetry_open.read() || *ctx.nav_open.read()) {
                    { let mut s = ctx.telemetry_open; *s.write() = false; }
                    { let mut s = ctx.nav_open; *s.write() = false; }
                }
            },
            style: "display: flex; flex-direction: column; flex: 1; min-height: 0; outline: none;",
            match ctx.screen.read().clone() {
                Screen::Boot => rsx! { BootSplash {} },
                Screen::BootError { detail } => rsx! { BootErrorScreen { detail: detail.clone() } },
                Screen::SeedVault { phrase, verify_indices, restore, has_identity } => rsx! {
                    crate::screens::SeedVaultScreen {
                        phrase: phrase.clone(),
                        verify_indices: verify_indices.clone(),
                        restore,
                        has_identity,
                    }
                },
                Screen::ByokSetup => rsx! { crate::screens::ByokSetupScreen {} },
                Screen::Canvas => rsx! { crate::screens::CanvasScreen {} },
                Screen::GoalCreate => rsx! { crate::screens::GoalCreateScreen {} },
                Screen::EveningCheckIn => rsx! { crate::screens::CheckInScreen {} },
                Screen::Dormant => rsx! { crate::screens::DormantScreen {} },
                Screen::MorningBrief => rsx! { crate::screens::MorningBriefScreen {} },
                Screen::Settings => rsx! { crate::screens::SettingsScreen {} },
            }
            if *ctx.nav_open.read() {
                crate::screens::NavDrawer {}
            }
            if *ctx.telemetry_open.read() {
                crate::screens::TelemetryDrawer {}
            }
            if let Some(msg) = ctx.toast.read().clone() {
                div { class: "wl-toast", "{msg}" }
            }
        }
    }
}

fn BootSplash() -> Element {
    rsx! {
        div { class: "wl-directive-container",
            h1 { class: "wl-brief-greeting", "Worldline" }
            p { class: "wl-body-muted", "Establishing cryptographic session…" }
        }
    }
}

/// Fail-closed boot (MVP-5): the shell could not answer. Never render
/// a fresh-looking empty Home over a broken vault — say what failed and
/// how to recover, without alarm red.
#[component]
fn BootErrorScreen(detail: String) -> Element {
    rsx! {
        div { class: "wl-directive-container",
            h1 { class: "wl-serif-title", "Storage could not be opened." }
            p { class: "wl-body-muted", style: "margin-top: 10px;",
                "Worldline refused to start with an unreadable vault or database rather than risk writing over your data."
            }
            p { class: "wl-body-muted wl-mono", style: "margin-top: 10px;", "{detail}" }
            p { class: "wl-body-muted", style: "margin-top: 14px;",
                "Recover by restoring your 12-word phrase, or wipe the application data directory to start over (this deletes local directives)."
            }
        }
    }
}

pub fn set_directive(ctx: &AppCtx, directive: Option<DirectiveView>) {
    // The session-timer bookkeeping that used to live here went with the
    // timer (see the note above `active_directive`); this is now just
    // the single-active filter plus the write.
    let mut current = ctx.directive;
    current.set(active_directive(directive));
}

pub fn flash(ctx: &AppCtx, msg: &str) {
    let mut toast = ctx.toast;
    let mut task = ctx.toast_task;
    if let Some(previous) = task.write().take() {
        previous.cancel();
    }
    toast.set(Some(msg.to_string()));
    task.set(Some(dioxus::core::spawn_forever(async move {
        gloo_timers::future::TimeoutFuture::new(2200).await;
        toast.set(None);
    })));
}

/// Shell `SyncStatsView` mirror (MVP-4). One DTO so every call site
/// (canvas, settings, telemetry, briefing) parses the same shape the
/// shell serializes.
#[derive(Clone, Debug, Default, serde::Deserialize, PartialEq)]
pub struct SyncStats {
    pub pushed: usize,
    pub pulled: usize,
    #[serde(default)]
    pub applied: usize,
    pub pending: usize,
    #[serde(default)]
    pub quarantined: usize,
    #[serde(default)]
    pub cursor: String,
}

impl SyncStats {
    /// Flash/toast copy: quarantined skips are named, never silent.
    pub fn summary(&self) -> String {
        if self.quarantined > 0 {
            format!(
                "SYNCED ↑{} ↓{} · {} quarantined",
                self.pushed, self.pulled, self.quarantined
            )
        } else {
            format!("SYNCED ↑{} ↓{}", self.pushed, self.pulled)
        }
    }
}

/// Records a successful sync cycle: HUD pill + freshness timestamp.
pub fn record_sync(ctx: &AppCtx, stats: &SyncStats) {
    let mut st = ctx.sync_status;
    if stats.pending > 0 {
        *st.write() = format!("{} PENDING", stats.pending);
    } else {
        *st.write() = "SYNCED".to_string();
    }
    let mut at = ctx.last_sync_ms;
    at.set(Some(js_sys::Date::now()));
}

/// HUD label for the current sync state, with age when we have one.
/// `LOCAL` means no relay session has completed yet — not a failure.
pub fn sync_label(ctx: &AppCtx) -> String {
    let status = ctx.sync_status.read().clone();
    let last = *ctx.last_sync_ms.read();
    match (status.as_str(), last) {
        ("OFFLINE", _) => "OFFLINE".to_string(),
        ("LOCAL", _) => "LOCAL · never synced".to_string(),
        (_, None) => status,
        (s, Some(ms)) => {
            let age = elapsed_secs(ms, js_sys::Date::now());
            format!("{s} · {age}s ago")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directive() -> DirectiveView {
        DirectiveView {
            directive_id: "first".into(),
            state: "active".into(),
            phase: Some((1, 2)),
            ..Default::default()
        }
    }

    /// `elapsed_secs` backs the sync-freshness label, not a timer: it must
    /// derive from timestamps (not tick counts) and clamp a backwards wall
    /// clock to zero rather than wrapping into a huge age.
    #[test]
    fn elapsed_uses_timestamps_not_ticks() {
        assert_eq!(elapsed_secs(1000.0, 1999.0), 0);
        assert_eq!(elapsed_secs(1000.0, 62000.0), 61);
        assert_eq!(elapsed_secs(1000.0, 500.0), 0);
        // Sub-second precision truncates, it does not round up.
        assert_eq!(elapsed_secs(0.0, 999.0), 0);
    }

    #[test]
    fn inactive_or_missing_directives_are_cleared() {
        assert!(active_directive(None).is_none());
        assert!(active_directive(Some(DirectiveView::default())).is_none());
        let mut d = directive();
        assert!(active_directive(Some(d.clone())).is_some());
        d.state = "idle".into();
        assert!(active_directive(Some(d.clone())).is_none());
        d.state = "active".into();
        d.directive_id.clear();
        assert!(active_directive(Some(d)).is_none());
    }

    #[test]
    fn idle_velocity_and_sync_defaults_are_honest() {
        // A quiet cycle reports what moved and never invents skips.
        let quiet = SyncStats {
            pushed: 0,
            pulled: 0,
            applied: 0,
            pending: 0,
            quarantined: 0,
            cursor: String::new(),
        };
        assert_eq!(quiet.summary(), "SYNCED ↑0 ↓0");
        let moved = SyncStats {
            pushed: 3,
            pulled: 2,
            ..quiet.clone()
        };
        assert_eq!(moved.summary(), "SYNCED ↑3 ↓2");
        // Quarantined skips are surfaced, never silent (MVP-4).
        let skipped = SyncStats {
            pushed: 1,
            pulled: 1,
            quarantined: 2,
            ..quiet.clone()
        };
        assert_eq!(skipped.summary(), "SYNCED ↑1 ↓1 · 2 quarantined");
        // Future/older shells omit the new fields — parse must not fail
        // (the DTO is the single wire contract for every call site).
        let legacy: SyncStats =
            serde_json::from_str(r#"{"pushed":1,"pulled":2,"applied":3,"pending":4}"#).unwrap();
        assert_eq!(
            legacy,
            SyncStats {
                pushed: 1,
                pulled: 2,
                applied: 3,
                pending: 4,
                quarantined: 0,
                cursor: String::new(),
            }
        );
    }
}
