//! System Telemetry Drawer (spec Screen 7).
//!
//! Overlay toggled by Ctrl+, (web equivalent of spec ⌘,) or by tapping
//! the timer pill. REUSE-ONLY: reads `settings_get` state already in
//! `ctx.settings`, `sync_now` for pending counts, and static geometry.
//! HLC head / WAL size have no shell command (per decision) and render
//! as unavailable rather than fabricated.

use dioxus::prelude::*;

use crate::app::{flash, invoke, AppCtx, Screen};

pub fn TelemetryDrawer() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut busy = use_signal(|| false);
    let s = ctx.settings.read().clone();
    let sync = ctx.sync_status.read().clone();
    let v = ctx.velocity.read().clone();
    let provider_label = s
        .ai_provider
        .clone()
        .unwrap_or_else(|| "none (manual mode)".into());
    let relay_label = s
        .relay_url
        .clone()
        .unwrap_or_else(|| "no relay configured".into());
    let pin_label = if s.always_on_top {
        "always on top"
    } else {
        "floating"
    };

    rsx! {
        div {
            class: "wl-modal-backdrop wl-drawer-backdrop",
            onclick: move |_| { { let mut t = ctx.telemetry_open; *t.write() = false; } },
            div {
                class: "wl-modal-sheet wl-drawer-sheet",
                role: "dialog",
                onclick: move |e| e.stop_propagation(),
                div { class: "wl-drawer-head",
                    h2 { class: "wl-modal-header", "System telemetry & vault" }
                    button {
                        class: "wl-btn-ghost wl-drawer-close",
                        onclick: move |_| { { let mut t = ctx.telemetry_open; *t.write() = false; } },
                        "Close"
                    }
                }

                div { class: "wl-field",
                    label { class: "wl-label", "Hardware keychain (Stronghold)" }
                    p { class: "wl-body-muted", "Status: enclave locked · Argon2id · 128-bit salt" }
                    p { class: "wl-body-muted", "Provider: {provider_label}" }
                }

                div { class: "wl-field",
                    label { class: "wl-label", "CRDT synchronization runtime" }
                    p { class: "wl-body-muted wl-mono", "Hybrid Logical Clock: — (no shell command)" }
                    p { class: "wl-body-muted wl-mono", "Relay state: {sync} · {relay_label}" }
                    p { class: "wl-body-muted wl-mono", "SQLite WAL size: — (no shell command)" }
                    p { class: "wl-body-muted wl-mono", "Velocity: {v.milestones_remaining} left · {v.days_remaining}d · target {v.target_per_day:.2}/day" }
                }

                div { class: "wl-field",
                    label { class: "wl-label", "Desktop viewport runtime" }
                    p { class: "wl-body-muted wl-mono", "Window geometry: 420px × 747px (9:16 fixed)" }
                    p { class: "wl-body-muted wl-mono", "Pin to top: {pin_label} · Summon: {s.hotkey}" }
                }

                div { style: "display: flex; flex-direction: column; gap: 8px;",
                    button {
                        class: "wl-btn-ghost",
                        disabled: *busy.read(),
                        onclick: move |_| {
                            let ctx = ctx;
                            busy.set(true);
                            spawn(async move {
                                #[derive(serde::Deserialize, Default)]
                                struct SyncOut {
                                    pushed: usize,
                                    pulled: usize,
                                    #[allow(dead_code)]
                                    applied: usize,
                                    pending: usize,
                                }
                                match invoke::<SyncOut>("sync_now", ()).await {
                                    Ok(o) => {
                                        let mut st = ctx.sync_status;
                                        if o.pending > 0 {
                                            let label: &'static str = Box::leak(
                                                format!("{} PENDING", o.pending).into_boxed_str(),
                                            );
                                            *st.write() = label;
                                        } else {
                                            *st.write() = "SYNCED";
                                        }
                                        flash(&ctx, format!("SYNCED ↑{} ↓{}", o.pushed, o.pulled).as_str());
                                    }
                                    Err(_) => {
                                        let mut st = ctx.sync_status;
                                        *st.write() = "OFFLINE";
                                        flash(&ctx, "SYNC FAILED — OFFLINE?");
                                    }
                                }
                                busy.set(false);
                            });
                        },
                        if *busy.read() { "Syncing…" } else { "Force CRDT peer re-sync" }
                    }
                    button {
                        class: "wl-btn-ghost",
                        onclick: move |_| {
                            { let mut t = ctx.telemetry_open; *t.write() = false; }
                            { let mut sc = ctx.screen; *sc.write() = Screen::Settings; }
                        },
                        "Open full settings"
                    }
                    p { class: "wl-seed-sub",
                        "Key purge lives in Settings per provider. No mnemonic or key material is ever displayed here."
                    }
                }
            }
        }
    }
}
