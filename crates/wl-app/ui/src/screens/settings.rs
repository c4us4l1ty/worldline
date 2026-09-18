//! Settings — theme, hotkey, always-on-top, BYOK provider/model ids,
//! relay URL. API KEYS themselves live ONLY in the Stronghold vault —
//! this screen configures *which* provider, never the secret.

use dioxus::prelude::*;

use crate::app::{flash, invoke, AppCtx, Screen};

pub fn SettingsScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut local = use_signal(|| ctx.settings.read().clone());
    let mut api_key = use_signal(String::new);
    let mut key_saved = use_signal(|| false);
    // Probe whether a key is already stored for the current provider.
    // Reads `local` inside the effect so provider switches re-probe;
    // the in-flight guard drops stale responses from rapid switches.
    use_effect(move || {
        let p = local.read().ai_provider.clone().unwrap_or_default();
        let still_current = local;
        let mut saved = key_saved;
        spawn(async move {
            if p.is_empty() {
                saved.set(false);
                return;
            }
            let has: bool = invoke("has_api_key", serde_json::json!({ "provider": p.clone() }))
                .await
                .unwrap_or(false);
            if still_current.peek().ai_provider.clone().unwrap_or_default() == p {
                saved.set(has);
            }
        });
    });
    let theme_controller = use_context::<crate::theme::Theme>();

    let save = move || {
        let ctx = ctx;
        let s = local.read().clone();
        spawn(async move {
            // Tauri maps top-level arg keys to command params, and the
            // shell's param is named `settings` — so the object must be
            // wrapped (a flat object would fail deserialization).
            match invoke::<serde_json::Value>(
                "settings_save",
                serde_json::json!({ "settings": s.clone() }),
            )
            .await
            {
                Ok(_) => {
                    {
                        let value = s.clone();
                        let mut sig = ctx.settings;
                        *sig.write() = value;
                    }
                    flash(&ctx, "SAVED");
                }
                Err(e) => flash(&ctx, &format!("ERR {e}")),
            }
        });
    };

    let s = local.read().clone();

    rsx! {
        div { class: "wl-scroll-region",
            h1 { class: "wl-serif-title", "Settings" }

            div { class: "wl-field", style: "margin-top: 18px;",
                label { class: "wl-label", "Theme" }
                select { class: "wl-select",
                    value: "{s.theme}",
                    onchange: move |e| {
                        let mut v = local.read().clone();
                        v.theme = e.value();
                        local.set(v);
                        theme_controller.set(e.value());
                    },
                    option { value: "dark", "Dark graphite (default)" }
                    option { value: "light", "Light parchment" }
                }
            }

            div { class: "wl-field",
                label { class: "wl-label", "Summon hotkey" }
                input { class: "wl-input", r#type: "text", value: "{s.hotkey}",
                    oninput: move |e| {
                        let mut v = local.read().clone();
                        v.hotkey = e.value();
                        local.set(v);
                    } }
            }

            div { class: "wl-field",
                label { class: "wl-label", "Pin to top" }
                button { class: "wl-btn-ghost",
                    onclick: move |_| {
                        let pinned = !local.read().always_on_top;
                        // Optimistic display flip; the preference only
                        // sticks when BOTH the window call and the persist
                        // succeed — boot restores the persisted value, so
                        // an unpersisted pin would silently unpin.
                        // Unsaved form edits are left untouched: the save
                        // payload flips only the pin on persisted state.
                        {
                            let mut cur = local;
                            let mut back = cur.read().clone();
                            back.always_on_top = pinned;
                            cur.set(back);
                        }
                        let ctx = ctx;
                        let mut base = ctx.settings.read().clone();
                        base.always_on_top = pinned;
                        spawn(async move {
                            let win_ok: Result<serde_json::Value, String> = invoke(
                                "set_always_on_top",
                                serde_json::json!({ "pinned": pinned }),
                            )
                            .await;
                            let save_ok: Result<serde_json::Value, String> = invoke(
                                "settings_save",
                                serde_json::json!({ "settings": base.clone() }),
                            )
                            .await;
                            if win_ok.is_ok() && save_ok.is_ok() {
                                let mut sig = ctx.settings;
                                *sig.write() = base;
                            } else {
                                let mut cur = local;
                                let mut back = cur.read().clone();
                                back.always_on_top = !pinned;
                                cur.set(back);
                                flash(&ctx, "PIN FAILED");
                            }
                        });
                    },
                    if s.always_on_top { "Pinned — always on top" } else { "Floating below other windows" }
                }
            }

            div { class: "wl-field",
                label { class: "wl-label", "AI provider (BYOK)" }
                select { class: "wl-select",
                    value: "{s.ai_provider.clone().unwrap_or_default()}",
                    onchange: move |e| {
                        let mut v = local.read().clone();
                        v.ai_provider = if e.value().is_empty() { None } else { Some(e.value()) };
                        local.set(v);
                    },
                    option { value: "", "None (manual mode)" }
                    option { value: "openai-compat", "OpenAI" }
                    option { value: "openrouter", "OpenRouter" }
                    option { value: "anthropic", "Anthropic" }
                    option { value: "gemini-compat", "Gemini" }
                }
            }

            div { class: "wl-field",
                label { class: "wl-label", "Tier-1 model (architect)" }
                input { class: "wl-input", r#type: "text",
                    placeholder: "e.g. gpt-4o / claude-sonnet (your choice)",
                    value: "{s.tier1_model.clone().unwrap_or_default()}",
                    oninput: move |e| {
                        let mut v = local.read().clone();
                        v.tier1_model = if e.value().is_empty() { None } else { Some(e.value()) };
                        local.set(v);
                    } }
            }

            div { class: "wl-field",
                label { class: "wl-label", "Tier-2 model (dispatcher)" }
                input { class: "wl-input", r#type: "text",
                    placeholder: "e.g. claude-haiku / gemini-flash",
                    value: "{s.tier2_model.clone().unwrap_or_default()}",
                    oninput: move |e| {
                        let mut v = local.read().clone();
                        v.tier2_model = if e.value().is_empty() { None } else { Some(e.value()) };
                        local.set(v);
                    } }
            }

            div { class: "wl-field",
                label { class: "wl-label", "API key (stored in vault only)" }
                input { class: "wl-input", r#type: "password",
                    placeholder: "Never synced, never in SQLite",
                    value: "{api_key.read().clone()}",
                    oninput: move |e| api_key.set(e.value()) }
                p { class: "wl-seed-sub", style: "margin-top: 6px;",
                    "Keys pass directly into the hardware-backed vault. They never touch the DOM, the database, or the relay."
                }
                div { style: "display: flex; gap: 8px; margin-top: 8px; align-items: center;",
                    button {
                        class: "wl-btn-ghost",
                        onclick: move |_| {
                            let ctx = ctx;
                            let provider = local
                                .read()
                                .ai_provider
                                .clone()
                                .unwrap_or_else(|| "openai-compat".into());
                            let key = api_key.read().clone();
                            if key.trim().is_empty() {
                                flash(&ctx, "KEY EMPTY");
                                return;
                            }
                            spawn(async move {
                                match invoke::<bool>(
                                    "set_api_key",
                                    serde_json::json!({ "provider": provider, "key": key }),
                                )
                                .await
                                {
                                    Ok(true) => {
                                        key_saved.set(true);
                                        api_key.set(String::new());
                                        flash(&ctx, "KEY SEALED IN VAULT");
                                    }
                                    _ => flash(&ctx, "KEY REJECTED"),
                                }
                            });
                        },
                        if *key_saved.read() { "Replace vault key" } else { "Save key to vault" }
                    }
                    if *key_saved.read() {
                        span { class: "wl-hud-pill", "sealed" }
                        button {
                            class: "wl-btn-ghost",
                            style: "color: var(--wl-accent-coral);",
                            onclick: move |_| {
                                let ctx = ctx;
                                let provider = local
                                    .read()
                                    .ai_provider
                                    .clone()
                                    .unwrap_or_else(|| "openai-compat".into());
                                spawn(async move {
                                    match invoke::<bool>(
                                        "delete_api_key",
                                        serde_json::json!({ "provider": provider }),
                                    )
                                    .await
                                    {
                                        Ok(true) => {
                                            key_saved.set(false);
                                            flash(&ctx, "VAULT KEY REMOVED");
                                        }
                                        _ => flash(&ctx, "KEY NOT FOUND"),
                                    }
                                });
                            },
                            "Remove"
                        }
                    }
                }
            }

            div { class: "wl-field",
                label { class: "wl-label", "Relay URL" }
                input { class: "wl-input", r#type: "text",
                    placeholder: "http://127.0.0.1:8080",
                    value: "{s.relay_url.clone().unwrap_or_default()}",
                    oninput: move |e| {
                        let mut v = local.read().clone();
                        v.relay_url = if e.value().is_empty() { None } else { Some(e.value()) };
                        local.set(v);
                    } }
            }

            div { style: "display: flex; flex-direction: column; gap: 8px; margin-top: 6px;",
                button { class: "wl-btn-primary", onclick: move |_| save(), "Save settings" }
                button { class: "wl-btn-ghost",
                    onclick: move |_| {
                        let ctx = ctx;
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
                                Ok(s) => {
                                    let mut st = ctx.sync_status;
                                    if s.pending > 0 {
                                        *st.write() = format!("{} PENDING", s.pending);
                                    } else {
                                        *st.write() = "SYNCED".to_string();
                                    }
                                    flash(
                                        &ctx,
                                        format!("SYNCED ↑{} ↓{}", s.pushed, s.pulled).as_str(),
                                    );
                                }
                                Err(_) => {
                                    let mut st = ctx.sync_status;
                                    *st.write() = "OFFLINE".to_string();
                                    flash(&ctx, "SYNC FAILED — OFFLINE?");
                                }
                            }
                        });
                    },
                    "Sync now"
                }
                button { class: "wl-btn-escape",
                    onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::Canvas; } },
                    "Back to the line"
                }
            }
        }
    }
}
