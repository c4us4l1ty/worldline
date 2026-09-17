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
    SeedVault {
        phrase: Vec<String>,
        verify_indices: Vec<usize>,
        restore: bool,
    },
    ByokSetup,
    Canvas,
    GoalCreate,
    EveningCheckIn,
    Dormant,
    MorningBrief,
    Settings,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimerSession {
    directive_id: String,
    phase: Option<(i64, i64)>,
    pub started_ms: f64,
}

impl TimerSession {
    fn update(
        previous: Option<Self>,
        directive: Option<&DirectiveView>,
        now_ms: f64,
    ) -> Option<Self> {
        let directive = directive?;
        if let Some(session) = previous {
            if session.directive_id == directive.directive_id && session.phase == directive.phase {
                return Some(session);
            }
        }
        Some(Self {
            directive_id: directive.directive_id.clone(),
            phase: directive.phase,
            started_ms: now_ms,
        })
    }
}

pub fn active_directive(directive: Option<DirectiveView>) -> Option<DirectiveView> {
    directive.filter(|d| d.state == "active" && !d.directive_id.is_empty())
}

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
    pub timer_session: Signal<Option<TimerSession>>,
    pub directive_busy: Signal<bool>,
    pub toast_task: Signal<Option<Task>>,
    pub escape_open: Signal<bool>,
    pub telemetry_open: Signal<bool>,
    pub sync_status: Signal<String>,
}

fn App() -> Element {
    use_context_provider(|| AppCtx {
        screen: Signal::new(Screen::Boot),
        directive: Signal::new(None),
        velocity: Signal::new(VelocityView::default()),
        settings: Signal::new(AppSettingsView::default()),
        toast: Signal::new(None),
        timer_session: Signal::new(None),
        directive_busy: Signal::new(false),
        toast_task: Signal::new(None),
        escape_open: Signal::new(false),
        telemetry_open: Signal::new(false),
        sync_status: Signal::new("LOCAL".to_string()),
    });

    let ctx = use_context::<AppCtx>();
    let _theme = use_context_provider(Theme::new);

    // Boot: check identity → route (runs once).
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
            let status: IdentityStatus = invoke("identity_status", ()).await.unwrap_or_default();
            let settings: AppSettingsView = invoke("settings_get", ()).await.unwrap_or_default();
            crate::theme::apply_data_theme(if settings.theme == "light" {
                "light"
            } else {
                "dark"
            });
            if status.has {
                if status.unlocked {
                    *screen.write() = Screen::Canvas;
                } else {
                    // Post-restart: try the vault copy before asking the
                    // user to re-enter the phrase (Item 3 unlock flow).
                    match invoke::<String>("identity_unlock", ()).await {
                        Ok(_) => *screen.write() = Screen::Canvas,
                        Err(_) => {
                            *screen.write() = Screen::SeedVault {
                                phrase: Vec::new(),
                                verify_indices: Vec::new(),
                                restore: true,
                            }
                        }
                    }
                }
            } else {
                *screen.write() = Screen::SeedVault {
                    phrase: Vec::new(),
                    verify_indices: Vec::new(),
                    restore: false,
                };
            }
            *settings_sig.write() = settings;
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
                } else if e.key() == Key::Escape && *ctx.telemetry_open.read() {
                    { let mut s = ctx.telemetry_open; *s.write() = false; }
                }
            },
            style: "display: flex; flex-direction: column; flex: 1; min-height: 0; outline: none;",
            match ctx.screen.read().clone() {
                Screen::Boot => rsx! { BootSplash {} },
                Screen::SeedVault { phrase, verify_indices, restore } => rsx! {
                    crate::screens::SeedVaultScreen {
                        phrase: phrase.clone(),
                        verify_indices: verify_indices.clone(),
                        restore,
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

pub fn fmt_mmss(secs: u64) -> String {
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

pub fn set_directive(ctx: &AppCtx, directive: Option<DirectiveView>) {
    let directive = active_directive(directive);
    let mut session = ctx.timer_session;
    let next = TimerSession::update(
        session.peek().clone(),
        directive.as_ref(),
        js_sys::Date::now(),
    );
    session.set(next);
    let mut current = ctx.directive;
    current.set(directive);
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

    #[test]
    fn elapsed_uses_timestamps_not_ticks() {
        assert_eq!(elapsed_secs(1000.0, 1999.0), 0);
        assert_eq!(elapsed_secs(1000.0, 62000.0), 61);
        assert_eq!(elapsed_secs(1000.0, 500.0), 0);
        assert_eq!(fmt_mmss(61), "01:01");
        assert_eq!(fmt_mmss(6000), "100:00");
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
    fn timer_session_survives_refresh_but_resets_for_new_work() {
        let mut d = directive();
        let session = TimerSession::update(None, Some(&d), 1000.0);
        d.title = "Updated title".into();
        let unchanged = TimerSession::update(session.clone(), Some(&d), 5000.0);
        assert_eq!(session, unchanged);
        d.phase = Some((2, 2));
        let next_phase = TimerSession::update(unchanged, Some(&d), 5000.0);
        assert_eq!(next_phase.as_ref().unwrap().started_ms, 5000.0);
        d.directive_id = "second".into();
        let next_directive = TimerSession::update(next_phase, Some(&d), 9000.0);
        assert_eq!(next_directive.as_ref().unwrap().started_ms, 9000.0);
        assert!(TimerSession::update(next_directive, None, 10000.0).is_none());
    }
}
