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


/// Records a fatal startup failure somewhere a GUI user can find it
/// and retitle the window so it does not look like a healthy app that
/// happens to be blank.
///
/// `AppState::new` is fatal on purpose: an unreadable store, a corrupt
/// snapshot, or a malformed `.vault-key` all mean the install cannot be
/// opened without losing the only copy of the mnemonic. `.expect` in
/// `main` turned that into a panic message on a console the user does
/// not have, in a window they were already looking at.
fn report_startup_failure(handle: &tauri::AppHandle, error: &ShellError) {
    let message = error.to_string();
    // The log is written to the app data dir, which is where the vault
    // and database live, so the path in the title is the path the user
    // already knows how to find.
    if let Ok(dir) = handle.path().app_data_dir() {
        let log = dir.join("startup-error.log");
        if let Err(e) = std::fs::write(&log, format!("{message}\n")) {
            eprintln!("could not write {}: {e}", log.display());
        }
    }
    eprintln!("Worldline shell failed to start: {message}");
    if let Some(win) = handle.get_webview_window("main") {
        let _ = win.set_title("Worldline — startup failed (see startup-error.log)");
    }
}

/// A binary built without `custom-protocol` loads `devUrl`
/// (`http://localhost:1420`) and nothing serves it, so the window shows
/// the *browser's* connection error page. The process is healthy and
/// every command would work; to a user it is indistinguishable from a
/// broken app. `build.rs` already warns at compile time, but warnings
/// scroll past and the person who needs this is the one running the
/// binary. Diagnostics only: no blocking, no fallback, no behaviour
/// change.
fn warn_dev_mode_loads_a_dev_server() {
    if cfg!(feature = "custom-protocol") {
        return;
    }
    eprintln!(
 "\n  Worldline was built WITHOUT `custom-protocol`.\n  \
 The window will load http://localhost:1420 (tauri.conf.json devUrl) and\n  \
 show \"Could not connect to localhost\" if nothing serves that port.\n  \
 That is the browser's error page, not a broken app.\n\n  \
 * daily use:  cargo tauri build --no-bundle, then run\n  \
               crates/wl-app/target/release/wl-app (embeds the bundle)\n  \
 * dev:        cargo tauri dev  (starts `dx serve --port 1420` for you)\n  \
 * plain cargo build/run is only valid while that dev server is already up.\n"
    );
}

fn main() {
    warn_dev_mode_loads_a_dev_server();
    add_commands(
        tauri::Builder::default()
            // NOTE: tauri-plugin-stronghold is intentionally NOT registered as
            // a plugin: its JS-side commands would expose a second,
            // zero-keyed vault to the webview. The shell manages its own
            // `vault::Vault` instance instead; the crate dependency remains
            // for the snapshot format + Stronghold type.
            .setup(|app| {
                let state = match AppState::new(app.handle()) {
                    Ok(state) => state,
                    Err(e) => {
                        // Stay up with a titled window instead of exiting:
                        // the UI's boot-failure screen then has something to
                        // render, and the log names the cause.
                        report_startup_failure(app.handle(), &e);
                        return Ok(());
                    }
                };
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
