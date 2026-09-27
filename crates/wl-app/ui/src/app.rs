//! Worldline UI (Dioxus) — the Stackelberg single-directive canvas.
//!
//! Enforces the worldline skill invariants:
//! * Exactly ONE directive rendered at any moment (skill §5 "Do").
//! * 9:16 fixed portrait layout (420×747 desktop, fluid mobile).
//! * ⌘+Enter completes; Escape opens the frictionful bailout modal.
//! * Zero future lists, zero streaks, zero alarm red.
//!
//! Every screen renders under `.wl-root` (a flex column owned by
//! `wl.css`), so a screen that needs a column declares it and not the
//! geometry. Two components used to repeat that geometry as an inline
//! `style` attribute, which is how the directive card ended up
//! mis-centred in exactly one of them.

use dioxus::{core::Task, prelude::*};
use serde::{de::DeserializeOwned, Serialize};

use crate::theme::Theme;

/// Entry point. Returns without launching if the mount point is already
/// claimed.
pub fn main() {
    if !claim_mount_point() {
        return;
    }
    launch(App);
}

/// Take exclusive ownership of the `#main` mount point.
///
/// This is not defensive padding — it fixes a failure that shipped
/// visibly. The dev server can execute the wasm module more than once
/// in a session (its hot-reload path re-imports the module on
/// rebuild). Each execution ran `launch(App)`, which built a SECOND
/// independent component tree inside the same `#main`: two roots, two
/// boot sequences, every shell command issued twice, and two sets of
/// in-flight async tasks writing into two different copies of the app
/// state. The window then shows two boot splashes that never resolve,
/// because the two trees' signal writes do not reach each other's
/// subscribers.
///
/// So execution is made idempotent: the first call claims the page and
/// empties `#main` (a stale tree from an earlier execution must not
/// survive underneath the live one), and every later call is a no-op.
/// A bookkeeping failure must never stop the app from starting, so the
/// catch claims the mount anyway rather than returning false.
fn claim_mount_point() -> bool {
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen(inline_js = r#"
    export function wl_claim_mount() {
      const KEY = "__wl_mounted__";
      try {
        if (window[KEY]) { return false; }
        window[KEY] = true;
        const host = document.getElementById("main");
        if (host) { host.replaceChildren(); }
        return true;
      } catch (e) {
        return true;
      }
    }
  "#)]
    extern "C" {
        fn wl_claim_mount() -> bool;
    }
    wl_claim_mount()
}

/// Which IPC transport the shim resolved: `native` when it reached
/// Tauri's injected `__TAURI_INTERNALS__`, `mock` when it fell back to
/// the browser harness, `none` when `invoke-shim.js` never loaded.
///
/// Surfaced on the boot-failure screen because "the window is showing
/// canned data" and "the window is showing my data" look identical
/// from the outside, and a silently-missing native bridge is exactly
/// the kind of fault that is otherwise invisible until a user
/// notices their work did not save.
pub fn transport() -> &'static str {
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen(inline_js = r#"
    export function wl_transport() {
      if (typeof window.wlInvoke !== "function") { return "none"; }
      try {
        const t = window.__TAURI_INTERNALS__;
        if (t && typeof t.invoke === "function") { return "native"; }
      } catch (e) { /* not a Tauri webview */ }
      return "mock";
    }
  "#)]
    extern "C" {
        fn wl_transport() -> String;
    }
    match wl_transport().as_str() {
        "native" => "native",
        "mock" => "mock",
        _ => "none",
    }
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

/// Moves focus to the compose field if it is on screen.
///
/// Separate from `autogrow_compose` because they are called at different
/// moments: focus wants to happen after the node exists and after a
/// re-render, height wants to happen after the value settles.
pub fn autofocus_compose() {
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen(inline_js = r#"
    export function wl_autofocus_compose() {
      try {
        const el = document.querySelector(".wl-compose");
        if (el && document.activeElement !== el) { el.focus(); }
      } catch (e) {}
    }
  "#)]
    extern "C" {
        fn wl_autofocus_compose();
    }
    wl_autofocus_compose();
}

/// Resizes the compose `<textarea>` to fit its content, so the field
/// grows with what you type instead of scrolling inside a fixed box.
///
/// The height has to be measured in the DOM — wrapped line count cannot
/// be computed in Rust from a CSS `font-size` and a column width, and
/// guessing produces a scrollbar on the second line. Collapsing to
/// `height: auto` and reading `scrollHeight` is the reliable way.
///
/// Targets `.wl-compose` by selector rather than taking a Dioxus element
/// ref: there is exactly one compose field, and a ref would have to be
/// threaded through the `oninput` closure and re-acquired every time the
/// busy state re-creates the node. A missing element is a no-op, so this
/// is safe to call from an effect that also runs on other screens.
pub fn autogrow_compose() {
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen(inline_js = r#"
    export function wl_autogrow_compose() {
      try {
        const el = document.querySelector(".wl-compose");
        if (!el) return;
        el.style.height = "auto";
        const h = el.scrollHeight;
        if (h > 0) { el.style.height = h + "px"; }
      } catch (e) {}
    }
  "#)]
    extern "C" {
        fn wl_autogrow_compose();
    }
    wl_autogrow_compose();
}

/// The global telemetry chord, `Ctrl+,` (the web stand-in for the spec's
/// `⌘,`).
///
/// Takes the event by reference and matches the `Key` enum rather than
/// comparing it to a literal. `Key::Character` owns a `String`, so
/// `e.key() == Key::Character(",".to_string())` allocated one on EVERY
/// keydown anywhere in the app — and this handler is on the root, so
/// "anywhere" is the compose field, the search box, and every settings
/// text input.
pub fn is_telemetry_chord(e: &KeyboardData) -> bool {
    e.modifiers().ctrl() && matches!(e.key(), Key::Character(c) if c == ",")
}

/// True when the focused element takes text entry.
///
/// A chord typed into a text field is the user typing, not a shortcut,
/// and the two must not be confused. `KeyboardData` in Dioxus 0.7 exposes
/// no event target — there is no `Event::target` to match on and no
/// `tag_name()` on a handle — so this asks the document what holds focus.
/// For any key a person produces, the focused element IS the target, so
/// this answers the same question.
///
/// Non-text `<input>` types are excluded: pressing Ctrl+, with a
/// checkbox focused is a shortcut, not a paste.
pub fn focus_is_text_entry() -> bool {
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen(inline_js = r#"
    export function wl_focus_is_text_entry() {
      try {
        const el = document.activeElement;
        if (!el) { return false; }
        const tag = (el.tagName || "").toUpperCase();
        if (tag === "TEXTAREA" || tag === "SELECT") { return true; }
        if (tag !== "INPUT") { return false; }
        const type = (el.getAttribute("type") || "text").toLowerCase();
        return ["checkbox","radio","button","submit","reset","file","range","color","image"]
          .indexOf(type) === -1;
      } catch (e) { return false; }
    }
  "#)]
    extern "C" {
        fn wl_focus_is_text_entry() -> bool;
    }
    wl_focus_is_text_entry()
}

/// Local calendar date as `YYYY-MM-DD`, matching the store's shape-exact
/// `check_date` (which rejects `2026-9-8` and anything else chrono would
/// otherwise accept).
pub fn today_local() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// `date` shifted by `days`, or `None` when the result is not
/// representable. `None` is a real answer here: the store validates dates
/// shape-exactly, so passing a malformed string would fail the write
/// rather than being coerced.
pub fn date_plus_days(date: &str, days: i64) -> Option<String> {
    let d = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    d.checked_add_signed(chrono::Duration::days(days))
        .map(|d| d.format("%Y-%m-%d").to_string())
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
    pub ai_provider: Option<String>,
    pub tier1_model: Option<String>,
    pub tier2_model: Option<String>,
    pub relay_url: Option<String>,
}

impl Default for AppSettingsView {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            ai_provider: None,
            tier1_model: None,
            tier2_model: None,
            relay_url: None,
        }
    }
}

/// One entry from a provider's live model catalog (`list_models`).
///
/// Mirrors `wl_core::ai::catalog::ModelInfo`. `context_length` and
/// `supports_response_format` are optional on the wire so an older
/// shell that omits them still deserializes.
#[derive(Clone, Debug, serde::Deserialize, PartialEq)]
pub struct ModelInfoView {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub context_length: Option<i64>,
    #[serde(default)]
    pub supports_response_format: Option<bool>,
}

/// The provider's catalog as returned by `list_models`.
///
/// `recommended` is a display ordering only — it is never auto-selected
/// from, and a model absent from it is still fully usable.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct ModelListView {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub models: Vec<ModelInfoView>,
    #[serde(default)]
    pub recommended: Vec<String>,
    #[serde(default)]
    pub cached: bool,
    #[serde(default)]
    pub truncated_from: Option<usize>,
}

/// One active goal with its milestone tally, for the drawer's
/// "Active Worldlines" readout (`list_goals`).
///
/// The counts are the shell's, not a UI computation: the store is the
/// only thing that knows what counts as complete after a merge, and a
/// progress bar that disagrees with the canvas is worse than none.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct GoalView {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub target_date: Option<String>,
    #[serde(default)]
    pub milestone_done: usize,
    #[serde(default)]
    pub milestone_total: usize,
}

/// One row of the Entropy Log (`entropy_log`).
///
/// `still_blocked` is the whole reason this is a flag and not a separate
/// list: a directive bailed out for `external_dependency` is parked in
/// `blocked` and never recovers (PRD delta 33), while the other two
/// reasons resolve on the spot — scope downsizes and requeues, energy
/// skips. So the blocked ones are the residue, and they get badged in
/// place rather than shown twice.
#[derive(Clone, Debug, Default, serde::Deserialize)]
pub struct EntropyView {
    pub id: String,
    #[serde(default)]
    pub goal_title: String,
    #[serde(default)]
    pub directive_id: String,
    #[serde(default)]
    pub directive_title: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub still_blocked: bool,
}

// ---------------------------------------------------------------------------
// App state & routing (screen enum — no URL routing needed for the
// single-window terminal)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
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
    },
    Canvas,
    GoalCreate,
    EveningCheckIn,
    Dormant,
    Settings,
    /// Control panel → System Telemetry. The escape-hatch ledger, read
    /// back: what was bailed out of, why, and which of those are still
    /// stuck. Opened from the drawer, never on its own.
    EntropyLog,
    /// Control panel → System Telemetry. Required velocity against
    /// observed, which used to be reachable only from inside the evening
    /// audit — i.e. after the fact, and only if you remembered to log.
    Trajectory,
}

/// Where a boot lands, given whether the shell answered.
///
/// A healthy install opens the Canvas — the one surface the product is
/// built around, and the only one that should open unprompted. A shell
/// that refused to answer must NOT fall through to a default screen:
/// that would render a fresh-looking install over a broken vault, which
/// is the failure this branch exists to prevent.
///
/// `Ok`/`Err` over a bool because the error text is the payload
/// `BootError` renders; routing and the message it shows are one
/// decision, not two.
pub fn boot_screen(identity: Result<(), &str>) -> Screen {
    match identity {
        Ok(()) => Screen::Canvas,
        Err(detail) => Screen::BootError {
            detail: detail.to_string(),
        },
    }
}

/// How long boot may take before the app stops waiting and says so.
/// Comfortably longer than a cold SQLite open plus a Stronghold unlock
/// on slow or encrypted storage, and short enough that a wedge reads as
/// a fault within seconds of attention rather than "still starting"
/// forever.
///
/// Both bounds are enforced at compile time rather than in a test. A
/// budget that is a constant does not need a runtime assertion — it
/// needs to be impossible to change into something wrong without the
/// build failing, which is the same treatment
/// `MAX_SEALED_OP_BYTES` gets in `wl-sync`.
pub const BOOT_TIMEOUT_MS: u32 = 15_000;
const _: () = assert!(
    BOOT_TIMEOUT_MS >= 5_000,
    "a cold SQLite open plus a Stronghold unlock must fit inside the boot budget"
);
const _: () = assert!(
    BOOT_TIMEOUT_MS <= 30_000,
    "a wedged shell must be reported well before a user gives up"
);

/// The boot-timeout message, as a pure function of its inputs.
///
/// Split from [`boot_timeout_detail`] so the wording is testable off a
/// wasm target — `transport()` is an imported JS function and cannot be
/// called from a host test.
pub fn boot_timeout_message(timeout_ms: u32, transport: &str) -> String {
    format!(
        "The shell did not answer within {}s (IPC transport: {}).",
        timeout_ms / 1000,
        transport
    )
}

/// What a timed-out boot reports. Names the transport it found, because
/// the most expensive fault to debug here is a window that looks alive
/// while it is quietly not talking to anything.
pub fn boot_timeout_detail() -> String {
    boot_timeout_message(BOOT_TIMEOUT_MS, transport())
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
    /// Bumped by the boot-failure screen's retry. The boot effect keys
    /// off it, so retrying is a state change rather than a remount —
    /// which is what lets a failed boot recover without the user
    /// relaunching the app.
    pub boot_attempt: Signal<u32>,
}

fn App() -> Element {
    // Every field is owned by `ScopeId::APP` — the root scope, which
    // lives for the whole app — rather than by whatever scope happens to
    // be current when `App` first renders.
    //
    // This matters because `AppCtx` is `Copy` and its signals are
    // written from two places that are NOT component scopes: detached
    // `spawn`ed tasks, which run at the runtime's root scope, and event
    // handlers. Dioxus warns about exactly this ("a Copy Value created
    // in ScopeId(3) … used in ScopeId(0)") because a value owned by a
    // short-lived scope can be dropped while something still holds a
    // copy. Here the owner IS the root scope, so the value outlives
    // every writer by construction and the warning is a false positive.
    //
    // Stating the owner explicitly — rather than inheriting it from
    // `App`'s scope, which happens to be the root today — makes that
    // guarantee deliberate, so it survives the refactor that would
    // actually introduce the bug: moving this construction into a
    // component with a shorter-lived scope.
    use_context_provider(|| AppCtx {
        screen: Signal::new_in_scope(Screen::Boot, ScopeId::APP),
        directive: Signal::new_in_scope(None, ScopeId::APP),
        velocity: Signal::new_in_scope(VelocityView::default(), ScopeId::APP),
        settings: Signal::new_in_scope(AppSettingsView::default(), ScopeId::APP),
        toast: Signal::new_in_scope(None, ScopeId::APP),
        directive_busy: Signal::new_in_scope(false, ScopeId::APP),
        toast_task: Signal::new_in_scope(None, ScopeId::APP),
        escape_open: Signal::new_in_scope(false, ScopeId::APP),
        telemetry_open: Signal::new_in_scope(false, ScopeId::APP),
        nav_open: Signal::new_in_scope(false, ScopeId::APP),
        sync_status: Signal::new_in_scope("LOCAL".to_string(), ScopeId::APP),
        last_sync_ms: Signal::new_in_scope(None, ScopeId::APP),
        identity_status: Signal::new_in_scope(IdentityStatus::default(), ScopeId::APP),
        boot_attempt: Signal::new_in_scope(0, ScopeId::APP),
    });

    let ctx = use_context::<AppCtx>();
    let _theme = use_context_provider(Theme::new);

    // Boot (MVP-5): settings/theme, then route straight to the Canvas.
    // No onboarding screens at boot: identity work (unlock, restore,
    // generate, 12-word phrase) lives in Settings. The ONLY boot
    // failure that blocks is the shell refusing to answer — a silent
    // default would disguise a broken vault as a fresh install, so the
    // routing decision lives in `boot_screen` and is pinned by a test
    // rather than being inlined here.
    //
    // Three properties this loop has to have, each of which it did not
    // have before:
    //
    // * **Bounded.** A watchdog fires regardless of what the shell
    //   does, so the worst case is a BootError with a retry rather
    //   than a splash that spins forever and says nothing. An
    //   unbounded boot has no failure state, which is what turned a
    //   wedged shell into an app that looked merely "still starting".
    // * **Independent steps.** Settings and identity used to be
    //   awaited in sequence, so a `settings_get` that never settled
    //   meant `identity_status` was never even attempted — one stuck
    //   call blocked the entire boot. They are now concurrent and
    //   independent; routing waits only on identity.
    // * **Retryable.** Keyed on `boot_attempt`, so retrying is a state
    //   change rather than a remount of the whole app.
    let mut booted_attempt = use_signal(|| u32::MAX);
    let mut boot_settled = use_signal(|| false);
    use_effect(move || {
        let attempt = *ctx.boot_attempt.read();
        // `peek` so arming the guard does not subscribe this effect to
        // its own bookkeeping and re-trigger it forever.
        if *booted_attempt.peek() == attempt {
            return;
        }
        *booted_attempt.write() = attempt;
        let ctx2 = ctx;
        spawn(async move {
            let mut screen = ctx2.screen;
            let mut identity = ctx2.identity_status;
            boot_settled.set(false);
            *screen.write() = Screen::Boot;

            // Arm the watchdog FIRST. Anything awaited below can hang,
            // and the watchdog is the only thing guaranteed to run.
            let watchdog = spawn({
                let mut screen = ctx2.screen;
                let settled = boot_settled;
                async move {
                    gloo_timers::future::TimeoutFuture::new(BOOT_TIMEOUT_MS).await;
                    if !*settled.read() {
                        *screen.write() = Screen::BootError {
                            detail: boot_timeout_detail(),
                        };
                    }
                }
            });

            // Settings/theme is cosmetic and tolerant: a failure here
            // must not stop the app from starting, so it runs alongside
            // identity rather than ahead of it.
            spawn({
                let mut settings = ctx2.settings;
                async move {
                    if let Ok(s) = invoke::<AppSettingsView>("settings_get", ()).await {
                        crate::theme::apply_data_theme(if s.theme == "light" {
                            "light"
                        } else {
                            "dark"
                        });
                        *settings.write() = s;
                    }
                }
            });

            let status = invoke::<IdentityStatus>("identity_status", ()).await;
            if let Ok(s) = &status {
                *identity.write() = s.clone();
            }
            *screen.write() = boot_screen(status.as_ref().map(|_| ()).map_err(String::as_str));
            boot_settled.set(true);
            watchdog.cancel();
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
                if is_telemetry_chord(&e) {
                    // Suppressed either way: the chord is claimed by this
                    // handler, so the browser must not also act on it.
                    e.prevent_default();
                    // …but not while a text field has focus. This handler
                    // is on the app root, so every `<input>` and
                    // `<textarea>` in the tree is a descendant and its
                    // keydowns bubble here — the compose box, the API
                    // key, the mnemonic, the relay URL. Ctrl+, typed into
                    // the compose field used to pop the telemetry sheet
                    // over the sentence being written.
                    if !focus_is_text_entry() {
                        let open = *ctx.telemetry_open.read();
                        { let mut s = ctx.telemetry_open; *s.write() = !open; }
                    }
                } else if e.key() == Key::Escape && (*ctx.telemetry_open.read() || *ctx.nav_open.read()) {
                    e.prevent_default();
                    { let mut s = ctx.telemetry_open; *s.write() = false; }
                    { let mut s = ctx.nav_open; *s.write() = false; }
                }
            },
            // Every screen renders inside an error boundary. A panic in
            // a component's render otherwise leaves the LAST GOOD DOM in
            // place, which for a boot-time panic means the splash stays
            // on screen forever and looks exactly like a slow start.
            // This turns "frozen with no explanation" into "failed,
            // here is why, here is the way out".
            //
            // The two drawers are inside it for the same reason and used
            // to be the exception that proved the rule false: they are
            // components, they read shell-supplied data (goal titles,
            // milestone tallies, the `{provider} · {n} models` line), and
            // a panic in either left the window frozen with no recovery
            // affordance — through the two most-used overlays in the app.
            ErrorBoundary {
                handle_error: move |error: ErrorContext| {
                    let detail = error
                        .error()
                        .map(|e| format!("{e}"))
                        .unwrap_or_else(|| "unknown render failure".to_string());
                    rsx! { RenderErrorScreen { detail } }
                },
                match ctx.screen.read().clone() {
                    Screen::Boot => rsx! { BootSplash {} },
                    Screen::BootError { detail } => rsx! { BootErrorScreen { detail: detail.clone() } },
                    Screen::SeedVault { phrase, verify_indices } => rsx! {
                        crate::screens::SeedVaultScreen {
                            phrase: phrase.clone(),
                            verify_indices: verify_indices.clone(),
                        }
                    },
                    Screen::Canvas => rsx! { crate::screens::CanvasScreen {} },
                    Screen::GoalCreate => rsx! { crate::screens::GoalCreateScreen {} },
                    Screen::EveningCheckIn => rsx! { crate::screens::CheckInScreen {} },
                    Screen::Dormant => rsx! { crate::screens::DormantScreen {} },
                    Screen::Settings => rsx! { crate::screens::SettingsScreen {} },
                    Screen::EntropyLog => rsx! { crate::screens::EntropyLogScreen {} },
                    Screen::Trajectory => rsx! { crate::screens::TrajectoryScreen {} },
                }
                if *ctx.nav_open.read() {
                    crate::screens::NavDrawer {}
                }
                if *ctx.telemetry_open.read() {
                    crate::screens::TelemetryDrawer {}
                }
            }
            if let Some(msg) = ctx.toast.read().clone() {
                div { class: "wl-toast", "{msg}" }
            }
        }
    }
}

/// Render-panic fallback. Reached only when a screen's render throws;
/// see the `ErrorBoundary` in `App`. Reloading the page is the honest
/// offer here — the panic may be in a screen's own state, and a
/// half-initialised component tree is not something to keep running.
#[component]
fn RenderErrorScreen(detail: String) -> Element {
    rsx! {
        div { class: "wl-directive-container",
            h1 { class: "wl-serif-title", "This screen could not be drawn." }
            p { class: "wl-body-muted", style: "margin-top: 10px;",
                "The interface hit an internal error while rendering. Your directives and goals are untouched in local storage."
            }
            p { class: "wl-body-muted wl-mono", style: "margin-top: 10px;", "{detail}" }
            button {
                class: "wl-btn-primary",
                style: "margin-top: 18px;",
                onclick: move |_| { reload_page(); },
                "Reload"
            }
        }
    }
}

/// Full page reload, for the render-error screen's one recovery action.
fn reload_page() {
    use wasm_bindgen::prelude::*;
    #[wasm_bindgen(inline_js = r#"
    export function wl_reload() { window.location.reload(); }
  "#)]
    extern "C" {
        fn wl_reload();
    }
    wl_reload()
}

/// Boot splash. Counts elapsed seconds on purpose: a static "starting…"
/// is indistinguishable from a frozen one, which is exactly how a
/// wedged shell presented as an app that was merely busy. The happy path
/// costs one 1 Hz timer for the few hundred milliseconds boot actually
/// takes.
///
/// The ticker is spawned during render and cancelled with the component,
/// not from inside an effect. `spawn` scopes a task to whatever scope is
/// current when it is called, and Dioxus 0.7 invokes an effect's closure
/// with NO scope pushed (`Effect::run` calls the boxed closure directly;
/// the scope is never re-entered), so a task spawned from an effect
/// closure is not reliably the component's to cancel. The old comment
/// asserted the opposite — "the loop dies with this effect the moment the
/// splash unmounts" — on the strength of a scoping guarantee the runtime
/// does not make. Owning the handle makes the lifetime a property of
/// this component instead of an inference about the scheduler.
#[component]
fn BootSplash() -> Element {
    let elapsed = use_signal(|| 0u32);
    let ticker = {
        let mut elapsed = elapsed;
        // Chained `TimeoutFuture`s rather than `IntervalStream`: one
        // timer type and no `futures` feature.
        spawn(async move {
            loop {
                gloo_timers::future::TimeoutFuture::new(1000).await;
                elapsed += 1;
            }
        })
    };
    use_drop(move || ticker.cancel());
    let secs = *elapsed.read();
    rsx! {
        div { class: "wl-directive-container",
            // `.wl-serif-title`, not a `.wl-brief-greeting` of its own.
            // That class went with the morning briefing and was left
            // dangling on this heading, so the one screen a user sees
            // while waiting rendered its wordmark at inherited body size
            // and weight — a splash with no wordmark. Reusing the existing
            // hero class is both correct and one fewer thing to invent.
            h1 { class: "wl-serif-title", "Worldline" }
            p { class: "wl-body-muted", "Establishing cryptographic session…" }
            p { class: "wl-body-muted wl-mono", style: "margin-top: 10px;",
                "{secs}s"
            }
            // Past a third of the budget, stop looking patient and say
            // the wait is abnormal — the watchdog is about to act.
            // `saturating_mul`: `secs` is a `u32` and `BOOT_TIMEOUT_MS` is
            // a constant somebody will eventually loosen. A release-mode
            // wrap here would flip the "taking longer than usual" warning
            // off, silently, for exactly the boot that needs it.
            if secs.saturating_mul(1000) >= BOOT_TIMEOUT_MS / 3 {
                p { class: "wl-body-muted", style: "margin-top: 10px;",
                    "Taking longer than usual. The shell is not answering; this will report what went wrong."
                }
            }
        }
    }
}

/// Fail-closed boot (MVP-5): the shell could not answer. Never render
/// a fresh-looking empty Home over a broken vault — say what failed and
/// how to recover, without alarm red.
///
/// Retry is first because it is what almost always works: a shell that
/// failed to answer once has usually failed transiently, and making the
/// user relaunch to find that out is a punishment, not a safeguard.
/// The transport is shown because "the window looks fine and is not
/// talking to anything" is otherwise invisible.
#[component]
fn BootErrorScreen(detail: String) -> Element {
    let ctx = use_context::<AppCtx>();
    let retry = move |_| {
        let mut attempt = ctx.boot_attempt;
        attempt += 1;
    };
    rsx! {
        div { class: "wl-directive-container",
            h1 { class: "wl-serif-title", "Storage could not be opened." }
            p { class: "wl-body-muted", style: "margin-top: 10px;",
                "Worldline refused to start rather than start wrong. Nothing has been written or changed."
            }
            p { class: "wl-body-muted wl-mono", style: "margin-top: 10px;", "{detail}" }
            p { class: "wl-body-muted wl-mono", style: "margin-top: 6px;",
                "IPC transport: {transport()}"
            }
            div { style: "display: flex; flex-direction: column; gap: 8px; margin-top: 18px;",
                button { class: "wl-btn-primary", onclick: retry, "Retry" }
                p { class: "wl-body-muted", style: "margin-top: 6px;",
                    "If retry does not help, restore your 12-word phrase, or wipe the application data directory to start over (this deletes local directives)."
                }
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

    /// The timeout message is the only thing a wedged boot has to say
    /// for itself, so it has to carry the two facts that make the
    /// failure diagnosable: how long it waited, and whether it was even
    /// talking to a shell. The budget's own bounds are enforced by
    /// `const _: () = assert!(…)` next to the constant, not here — a
    /// constant does not need a runtime assertion to be checked.
    #[test]
    fn boot_failure_message_names_the_budget_and_the_transport() {
        let detail = boot_timeout_message(BOOT_TIMEOUT_MS, "native");
        // The message has to name the transport: a window that looks
        // alive while it is not talking to anything is otherwise
        // undiagnosable from the outside.
        assert!(
            detail.contains("transport") && detail.contains("native"),
            "boot timeout detail must name the transport: {detail}"
        );
        assert!(
            detail.contains(&(BOOT_TIMEOUT_MS / 1000).to_string()),
            "boot timeout detail must state the budget: {detail}"
        );
        // The three transports are distinguishable in the output, which
        // is the whole diagnostic value: "mock" means the shell bridge
        // is missing and nothing you do is being saved.
        for t in ["native", "mock", "none"] {
            assert!(
                boot_timeout_message(BOOT_TIMEOUT_MS, t).contains(t),
                "transport {t} must be visible in the failure message"
            );
        }
    }

    /// A shell that answers is the only thing that opens the canvas, and
    /// a shell that does not must never be papered over. Both halves,
    /// because the second is the fail-closed rule the whole screen
    /// exists to enforce.
    #[test]
    fn boot_routes_to_canvas_or_fails_closed_never_silently() {
        assert_eq!(boot_screen(Ok(())), Screen::Canvas);
        let failed = boot_screen(Err("vault: unreadable snapshot"));
        match failed {
            Screen::BootError { detail } => {
                assert_eq!(detail, "vault: unreadable snapshot")
            }
            other => panic!("a failed boot must not render {other:?}"),
        }
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

    /// A healthy install opens the Canvas and nothing else; a shell that
    /// refuses to answer must surface the failure instead of falling
    /// through to a default screen, which would render a fresh-looking
    /// install over a broken vault. The error text is carried through
    /// rather than discarded, because `BootError` shows it verbatim.
    #[test]
    fn boot_opens_the_canvas_and_never_masks_a_dead_shell() {
        assert_eq!(boot_screen(Ok(())), Screen::Canvas);
        assert_eq!(
            boot_screen(Err("vault: unreadable snapshot")),
            Screen::BootError {
                detail: "vault: unreadable snapshot".into()
            }
        );
    }
}
