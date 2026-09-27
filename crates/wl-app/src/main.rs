//! Worldline Tauri shell — the native host.
//!
//! Responsibilities (PRD §2.2, §3):
//! * Fixed 420×747 9:16 window, non-resizable, non-maximizable.
//! * Stronghold vault for the mnemonic + BYOK keys (never in DOM).
//! * #[tauri::command] thin layer over wl-core; SQLite stays native.
//!
//! Every command is a request/response call the UI awaits; the shell
//! emits no events. Sync and the model catalog are on-demand, and the
//! UI drives its HUD from the values those commands return.
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
        __temp::diag_sink,
    ])
}

/// TEMPORARY diagnostic scaffolding — remove before commit.
mod __temp {
    use std::io::Write;

    #[tauri::command(rename_all = "snake_case")]
    pub(crate) fn diag_sink(text: String) {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("/tmp/opencode/wldiag.txt")
        {
            let _ = writeln!(f, "{text}");
        }
    }

    const PROBE: &str = r#"
(function () {
  var out = [];
  function say(s) { out.push(String(s)); }
  say('ua=' + navigator.userAgent);
  say('transport=' + (typeof window.wlInvoke));
  window.addEventListener('error', function (e) { say('ERR ' + (e.message||e)); }, true);
  function d(el) {
    if (!el) return 'null';
    var s = el.tagName.toLowerCase();
    if (el.id) s += '#' + el.id;
    if (el.className && typeof el.className === 'string' && el.className.trim()) s += '.' + el.className.trim().split(/\s+/).join('.');
    return s;
  }
  document.addEventListener('click', function (e) {
    say('REALCLICK target=' + d(e.target) + ' trusted=' + e.isTrusted + ' x=' + Math.round(e.clientX) + ' y=' + Math.round(e.clientY));
  }, true);
  document.addEventListener('pointerdown', function (e) {
    say('PTRDOWN target=' + d(e.target));
  }, true);
  var n = 0;
  var iv = setInterval(function () {
    n++;
    var btn = document.querySelector('.wl-float-menu');
    if (!btn) { if (n > 80) { say('TIMEOUT no hamburger'); window.__TAURI_INTERNALS__.invoke('diag_sink', { text: out.join('\n') }); } return; }
    clearInterval(iv);
    var r = btn.getBoundingClientRect();
    var cs = getComputedStyle(btn), ls = getComputedStyle(btn.parentElement);
    say('rect=' + JSON.stringify({x:r.x,y:r.y,w:r.width,h:r.height}));
    say('btn pe=' + cs.pointerEvents + ' pe-events=' + cs.pointerEvents + ' pos=' + cs.position + ' z=' + cs.zIndex + ' vis=' + cs.visibility + ' disp=' + cs.display);
    say('layer=' + d(btn.parentElement) + ' pe=' + ls.pointerEvents + ' pos=' + ls.position + ' z=' + ls.zIndex);
    var cx = r.x + r.width / 2, cy = r.y + r.height / 2;
    say('hit centre=(' + cx + ',' + cy + ') -> ' + d(document.elementFromPoint(cx, cy)));
    say('hit tl=(' + (r.x+2) + ',' + (r.y+2) + ') -> ' + d(document.elementFromPoint(r.x+2, r.y+2)));
    say('btn data-dioxus-id=' + btn.getAttribute('data-dioxus-id'));
    say('svg data-dioxus-id=' + (btn.querySelector('svg') && btn.querySelector('svg').getAttribute('data-dioxus-id')));
    say('main data-dioxus-id=' + document.getElementById('main').getAttribute('data-dioxus-id'));
    var chain = [], q = btn;
    while (q && q !== document.documentElement) {
      var c = getComputedStyle(q);
      chain.push(d(q) + '{pos:' + c.position + ',z:' + c.zIndex + ',pe:' + c.pointerEvents + '}');
      q = q.parentElement;
    }
    say('chain: ' + chain.join(' < '));
    say('docElementFromPoint centre -> ' + d(document.elementFromPoint(cx, cy)));
    say('--- synthetic click ---');
    try { btn.click(); } catch (e) { say('click threw ' + e); }
    setTimeout(function () {
      say('after btn.click(): sheet=' + !!document.querySelector('.wl-nav-sheet') + ' backdrop=' + !!document.querySelector('.wl-nav-backdrop'));
      say('body text len=' + (document.body.innerText || '').length);
      var sb = document.querySelector('.wl-nav-sheet');
      if (sb) { var sr = sb.getBoundingClientRect(); say('sheet rect=' + JSON.stringify({x:sr.x,y:sr.y,w:sr.width,h:sr.height})); }
      say('--- synthetic path click ---');
      var p = btn.querySelector('path') || btn.querySelector('svg');
      if (p) p.dispatchEvent(new MouseEvent('click', {bubbles:true, cancelable:true, view:window}));
      setTimeout(function () {
        say('after path click: sheet=' + !!document.querySelector('.wl-nav-sheet'));
        window.__TAURI_INTERNALS__.invoke('diag_sink', { text: out.join(' || ') });
      }, 500);
    }, 700);
  }, 300);
})();
"#;

    pub fn spawn(win: tauri::WebviewWindow) {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(6));
            let _ = win.eval(PROBE);
        });
    }
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

fn main() {
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
                    __temp::spawn(win.clone());
                }
                Ok(())
            }),
    )
    .run(tauri::generate_context!())
    .expect("Worldline shell failed to start");
}
