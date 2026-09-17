//! Worldline Tauri shell — the native host.
//!
//! Responsibilities (PRD §2.2, §3):
//! * Fixed 420×747 9:16 window, non-resizable, non-maximizable.
//! * Global hotkey (default Alt+Space) to summon/dismiss.
//! * Stronghold vault for the mnemonic + BYOK keys (never in DOM).
//! * #[tauri::command] thin layer over wl-core; SQLite stays native.
//! * Emits engine outcomes as events for the Dioxus UI.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app_state;
mod commands;
mod error;
mod hotkey;
mod relay;
mod vault;

pub use error::{ShellError, ShellResult};

use std::sync::Arc;

use app_state::AppState;
use tauri::Manager;

fn main() {
    tauri::Builder::default()
        // NOTE: tauri-plugin-stronghold is intentionally NOT registered as
        // a plugin: its JS-side commands would expose a second,
        // zero-keyed vault to the webview. The shell manages its own
        // `vault::Vault` instance instead; the crate dependency remains
        // for the snapshot format + Stronghold type.
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            let state = AppState::new(app.handle())?;
            // Summon hotkey comes from persisted settings (PRD §2.2).
            let (hotkey, pinned) = state
                .repos
                .settings()
                .map(|s| (s.hotkey, s.always_on_top))
                .unwrap_or_else(|_| (hotkey::DEFAULT_HOTKEY.to_string(), false));
            app.manage(Arc::new(state));
            hotkey::register_summon_hotkey(app.handle(), &hotkey);
            // Dark boot background: the webview paints #131312 before the
            // first WASM frame instead of WebKit's white default, so the
            // fixed 9:16 window never flashes a blank white page.
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_background_color(Some(tauri::window::Color {
                    red: 19,
                    green: 19,
                    blue: 18,
                    alpha: 255,
                }));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::identity_generate,
            commands::identity_verify_backup,
            commands::identity_restore,
            commands::identity_status,
            commands::identity_unlock,
            commands::set_api_key,
            commands::has_api_key,
            commands::delete_api_key,
            commands::create_goal,
            commands::create_manual_milestone,
            commands::create_manual_directive,
            commands::current_directive,
            commands::complete_directive,
            commands::bail_out,
            commands::check_in,
            commands::velocity,
            commands::settings_get,
            commands::settings_save,
            commands::relay_authenticate,
            commands::sync_now,
            commands::morning_briefing,
            commands::master_plan,
            commands::set_always_on_top,
            commands::toggle_window_visibility,
        ])
        .run(tauri::generate_context!())
        .expect("Worldline shell failed to start");
}
