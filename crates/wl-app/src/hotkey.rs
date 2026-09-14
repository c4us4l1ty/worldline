//! Global summon hotkey (PRD §2.2, Item 4): Alt+Space by default,
//! configurable via settings. Registration reads the persisted hotkey
//! at boot and re-registers whenever settings change.

use tauri::{AppHandle, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

/// Default summon combination (PRD §2.2).
pub const DEFAULT_HOTKEY: &str = "alt+space";

/// (Re)registers the summon hotkey, replacing any previous binding.
/// Never fatal: a conflicting or malformed accelerator logs and leaves
/// the app running (the window stays reachable via taskbar/dock).
pub fn register_summon_hotkey(app: &AppHandle, hotkey: &str) {
    let hotkey = hotkey.trim();
    let hotkey = if hotkey.is_empty() {
        DEFAULT_HOTKEY
    } else {
        hotkey
    };
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    if let Err(e) = gs.on_shortcut(hotkey, |app, _shortcut, event| {
        if event.state == ShortcutState::Pressed {
            toggle_window_visibility(app);
        }
    }) {
        eprintln!("global hotkey {hotkey:?} unavailable: {e}");
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
