//! Worldline Tauri shell — the native host.
//!
//! Responsibilities (PRD §2.2, §3):
//! * Fixed 420×747 9:16 window, non-resizable, non-maximizable.
//! * Stronghold vault for the mnemonic + BYOK keys (never in DOM).
//! * #[tauri::command] thin layer over wl-core; SQLite stays native.
//! * Emits engine outcomes as events for the Dioxus UI.
//!
//! There is no global summon hotkey: the feature was removed, along with
//! `tauri-plugin-global-shortcut` and the `toggle_window_visibility`
//! command that only existed to serve it. The window stays reachable
//! through the taskbar (`skipTaskbar: false` in tauri.conf.json).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_state;
mod commands;
mod error;
mod relay;
mod vault;

#[cfg(test)]
mod ipc_tests;

pub use error::{ShellError, ShellResult};

use std::sync::Arc;

use app_state::AppState;
use tauri::Manager;

/// Registers the full command surface on a builder.
///
/// Extracted from `main` so the IPC contract test in `ipc_tests.rs` can
/// build a mock app over the SAME `generate_handler!` the real app
/// registers. A test that hand-copied the list would keep passing while
/// the app shipped a different one — which is the shape of bug this
/// exists to catch.
fn add_commands<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.invoke_handler(tauri::generate_handler![
        commands::identity_generate,
        commands::identity_verify_backup,
        commands::identity_restore,
        commands::identity_status,
        commands::identity_unlock,
        commands::set_api_key,
        commands::has_api_key,
        commands::delete_api_key,
        commands::create_goal,
        commands::list_goals,
        commands::entropy_log,
        commands::current_directive,
        commands::complete_directive,
        commands::bail_out,
        commands::check_in,
        commands::velocity,
        commands::settings_get,
        commands::settings_save,
        commands::relay_authenticate,
        commands::sync_now,
        commands::master_plan,
        commands::list_models,
    ])
}

fn main() {
    add_commands(
        tauri::Builder::default()
            // NOTE: tauri-plugin-stronghold is intentionally NOT registered as
            // a plugin: its JS-side commands would expose a second,
            // zero-keyed vault to the webview. The shell manages its own
            // `vault::Vault` instance instead; the crate dependency remains
            // for the snapshot format + Stronghold type.
            .setup(|app| {
                let state = AppState::new(app.handle())?;
                app.manage(Arc::new(state));
                // Dark boot background: the webview paints #131312 before the
                // first WASM frame instead of WebKit's white default, so the
                // fixed 9:16 window never flashes a blank white page.
                //
                // This handle used to also restore a persisted
                // always-on-top preference. That setting is gone with its
                // switch (PRD delta 175), so there is nothing to restore and
                // the window is a plain floating window from here on.
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.set_background_color(Some(tauri::window::Color(19, 19, 18, 255)));
                }
                Ok(())
            }),
    )
    .run(tauri::generate_context!())
    .expect("Worldline shell failed to start");
}
