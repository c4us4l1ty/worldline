//! Shell event emissions: pushes engine outcomes to the UI over Tauri
//! channels (e.g. timer ticks for the HUD countdown).

use std::time::Duration;

use tauri::Emitter;

/// Spawns a background ticker emitting `hud-tick` every second so the
/// UI's monospace timer stays live without owning a scheduler.
pub fn register_emit_timer(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            if let Err(e) = app.emit("hud-tick", ()) {
                eprintln!("hud-tick emit failed: {e}");
            }
        }
    });
}
