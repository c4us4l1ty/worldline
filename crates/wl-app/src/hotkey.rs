//! Global summon hotkey (PRD §2.2, Item 4): Alt+Space by default,
//! configurable via settings. Registration reads the persisted hotkey
//! at boot and re-registers whenever settings change.

use std::str::FromStr;

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

/// Default summon combination (PRD §2.2).
pub const DEFAULT_HOTKEY: &str = "alt+space";

/// (Re)registers the summon hotkey, replacing any previous binding.
/// Never fatal: a conflicting or malformed accelerator logs and leaves
/// the app running (the window stays reachable via taskbar/dock).
/// Parse failures never unregister the current binding, and a failed
/// registration falls back to `DEFAULT_HOTKEY` so a bad settings value
/// can never leave the app with NO summon hotkey until restart (B-007).
pub fn register_summon_hotkey(app: &AppHandle, hotkey: &str) {
    let hotkey = hotkey.trim();
    let hotkey = if hotkey.is_empty() {
        DEFAULT_HOTKEY
    } else {
        hotkey
    };
    // Validate BEFORE unregistering: a malformed accelerator must not
    // kill the existing binding.
    if Shortcut::from_str(hotkey).is_err() {
        eprintln!("global hotkey {hotkey:?} malformed: keeping current binding");
        ensure_default_registered(app);
        return;
    }
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    if let Err(e) = gs.on_shortcut(hotkey, on_summon) {
        eprintln!("global hotkey {hotkey:?} unavailable: {e}");
        // Registration failed (conflict/OS rejection): restore the
        // default so summon survives without a restart.
        if hotkey != DEFAULT_HOTKEY {
            if let Err(e) = gs.on_shortcut(DEFAULT_HOTKEY, on_summon) {
                eprintln!("default global hotkey unavailable: {e}");
            }
        }
    }
}

/// Summon handler shared by every registration path (fn item so it can
/// be passed to `on_shortcut` more than once per call).
fn on_summon(app: &AppHandle, _shortcut: &Shortcut, event: ShortcutEvent) {
    if event.state == ShortcutState::Pressed {
        toggle_window_visibility(app);
    }
}

/// Best-effort fallback: register the default summon hotkey unless it
/// is already registered. Used when the requested accelerator is
/// malformed and we refused to unregister the current binding.
fn ensure_default_registered(app: &AppHandle) {
    let gs = app.global_shortcut();
    if !gs.is_registered(DEFAULT_HOTKEY) {
        if let Err(e) = gs.on_shortcut(DEFAULT_HOTKEY, on_summon) {
            eprintln!("default global hotkey unavailable: {e}");
        }
    }
}

/// Shows the main window if hidden, hides it if visible. Shared by the
/// hotkey handler and the `toggle_window_visibility` command so both
/// paths behave identically.
pub fn toggle_window_visibility(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        match win.is_visible() {
            Ok(true) => {
                let _ = win.hide();
            }
            _ => {
                let _ = win.show();
                let _ = win.set_focus();
            }
        }
    }
}
