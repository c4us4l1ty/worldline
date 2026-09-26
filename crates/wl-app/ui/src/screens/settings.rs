//! Settings — theme, hotkey, always-on-top, BYOK provider/model ids,
//! relay URL. API KEYS themselves live ONLY in the Stronghold vault —
//! this screen configures *which* provider, never the secret.

use dioxus::prelude::*;

use crate::app::{flash, invoke, record_sync, AppCtx, IdentityStatus, Screen, SyncStats};

pub fn SettingsScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut local = use_signal(|| ctx.settings.read().clone());
    let mut api_key = use_signal(String::new);
    let mut key_saved = use_signal(|| false);
    // MVP-4: guards the relay connection test (single in-flight probe).
    let mut busy = use_signal(|| false);
    // MVP-5: identity management lives here (not at boot). The phrase
    // input is write-only and cleared as soon as the shell accepts it.
    let mut phrase = use_signal(String::new);
    let mut identity_busy = use_signal(|| false);
    let mut identity_error = use_signal(String::new);
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
    let status = ctx.identity_status.read().clone();

    rsx! {
        div { class: "wl-page",
            // The way back used to be a button at the very bottom of a long
            // page, styled `.wl-btn-escape` — which reads as the bailout
            // affordance, not navigation. It now lives in a fixed header,
            // in the same place as every other page.
            div { class: "wl-page-head",
                button {
                    class: "wl-back",
                    aria_label: "Back to the line",
                    title: "Back to the line",
                    onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::Canvas; } },
                    "\u{2190}"
                }
                div {
                    h1 { class: "wl-page-title", "Settings" }
                    p { class: "wl-page-sub",
                        "Secrets stay in the local vault. This screen only points the app at them."
                    }
                }
            }

            div { class: "wl-scroll-region",
            // MVP-5: identity (12-word phrase) lives in Settings.
            // Boot never opens onboarding; this is the only place that
            // unlocks, restores, or generates the phrase.
            div { class: "wl-section",
                span { class: "wl-section-label", "Identity" }
                p { class: "wl-section-note wl-mono",
                    if !status.has {
                        "no identity on this install"
                    } else if status.unlocked {
                        "unlocked · phrase held in memory"
                    } else if status.vault_has_mnemonic {
                        "locked · phrase sealed in the vault"
                    } else {
                        "locked · no vault copy — restore from your 12 words"
                    }
                }
                if !status.has {
                    button {
                        class: "wl-btn-ghost",
                        style: "width: auto; padding: 0 16px;",
                        disabled: *identity_busy.read(),
                        onclick: move |_| {
                            {
                                let mut s = ctx.screen;
                                *s.write() = Screen::SeedVault {
                                    phrase: Vec::new(),
                                    verify_indices: Vec::new(),
                                    restore: false,
                                    has_identity: false,
                                };
                            };
                        },
                        "Generate a new identity"
                    }
                } else if !status.unlocked {
                    if status.vault_has_mnemonic {
                        button {
                            class: "wl-btn-ghost",
                            style: "width: auto; padding: 0 16px; margin-bottom: 10px;",
                            disabled: *identity_busy.read(),
                            onclick: move |_| {
                                let ctx = ctx;
                                identity_busy.set(true);
                                identity_error.set(String::new());
                                spawn(async move {
                                    match invoke::<String>("identity_unlock", ()).await {
                                        Ok(_) => {
                                            let st: IdentityStatus =
                                                invoke("identity_status", ()).await.unwrap_or_default();
                                            { let mut sig = ctx.identity_status; sig.set(st); }
                                            flash(&ctx, "IDENTITY UNLOCKED");
                                        }
                                        Err(e) => identity_error.set(e),
                                    }
                                    identity_busy.set(false);
                                });
                            },
                            if *identity_busy.read() { "Unlocking…" } else { "Unlock from vault" }
                        }
                    }
                    div { style: "display: flex; gap: 8px; align-items: center;",
                        input {
                            class: "wl-input wl-mono",
                            r#type: "password",
                            autocomplete: "off",
                            spellcheck: "false",
                            placeholder: "12 words, space-separated",
                            value: "{phrase.read().clone()}",
                            oninput: move |e| phrase.set(e.value()),
                        }
                        button {
                            class: "wl-btn-ghost",
                            style: "width: auto; padding: 0 16px; flex-shrink: 0;",
                            disabled: *identity_busy.read(),
                            onclick: move |_| {
                                let ctx = ctx;
                                let words = phrase.read().clone();
                                if words.split_whitespace().count() != 12 {
                                    identity_error.set("Enter all 12 words, space-separated.".into());
                                    return;
                                }
                                identity_busy.set(true);
                                identity_error.set(String::new());
                                spawn(async move {
                                    match invoke::<String>(
                                        "identity_restore",
                                        serde_json::json!({ "phrase": words }),
                                    )
                                    .await
                                    {
                                        Ok(_) => {
                                            // Secret hygiene: the phrase
                                            // never lingers in the DOM.
                                            phrase.set(String::new());
                                            let st: IdentityStatus =
                                                invoke("identity_status", ()).await.unwrap_or_default();
                                            { let mut sig = ctx.identity_status; sig.set(st); }
                                            flash(&ctx, "IDENTITY RESTORED");
                                        }
                                        Err(e) => identity_error.set(e),
                                    }
                                    identity_busy.set(false);
                                });
                            },
                            "Restore"
                        }
                    }
                    if !identity_error.read().is_empty() {
                        p { class: "wl-seed-sub", style: "margin-top: 6px; color: var(--wl-accent-coral);",
                            "{identity_error.read().clone()}"
                        }
                    }
                }
                p { class: "wl-section-note", style: "margin-top: 10px;",
                    "The phrase is never written to SQLite or the relay. Local directives work without it; sync and AI keys need it unlocked."
                }
            }

            div { class: "wl-section",
                span { class: "wl-section-label", "Appearance" }
                div { class: "wl-field",
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
                    input { class: "wl-input", r#type: "text", placeholder: "alt+space",
                        value: "{s.hotkey}",
                        oninput: move |e| {
                            let mut v = local.read().clone();
                            v.hotkey = e.value();
                            local.set(v);
                        } }
                }
                div { class: "wl-field",
                    label { class: "wl-label", "Window" }
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
            }

            div { class: "wl-section",
                span { class: "wl-section-label", "Intelligence" }
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
                        option { value: "openrouter", "OpenRouter" }
                        option { value: "google", "Google" }
                        option { value: "qwen", "Qwen" }
                        option { value: "bytez.com", "bytez.com" }
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
                    label { class: "wl-label", "API key" }
                    input { class: "wl-input", r#type: "password",
                        placeholder: "Sealed into the local vault",
                        autocomplete: "off",
                        spellcheck: "false",
                        value: "{api_key.read().clone()}",
                        oninput: move |e| api_key.set(e.value()) }
                    div { class: "wl-inline-actions",
                        button {
                            class: "wl-btn-ghost",
                            onclick: move |_| {
                                let ctx = ctx;
                                let provider = local
                                    .read()
                                    .ai_provider
                                    .clone()
                                    .unwrap_or_else(|| "openrouter".into());
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
                            span { class: "wl-chip", "sealed" }
                            button {
                                class: "wl-btn-ghost",
                                style: "color: var(--wl-accent-coral); flex: 0 0 auto;",
                                onclick: move |_| {
                                    let ctx = ctx;
                                    let provider = local
                                        .read()
                                        .ai_provider
                                        .clone()
                                        .unwrap_or_else(|| "openrouter".into());
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
                    p { class: "wl-section-note", style: "margin-top: 10px;",
                        "Sealed into the local Stronghold vault, then cleared from this field. Never written to SQLite, never sent to the relay."
                    }
                }
            }

            div { class: "wl-section",
                span { class: "wl-section-label", "Sync" }
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
                    div { class: "wl-inline-actions",
                        button { class: "wl-btn-ghost",
                            disabled: *busy.read(),
                            onclick: move |_| {
                                // MVP-4: explicit handshake test — the docstring
                                // promised one, nothing called it. Uses the
                                // PERSISTED relay URL (the shell reads its own
                                // copy), so unsaved edits must be saved first.
                                let ctx = ctx;
                                busy.set(true);
                                spawn(async move {
                                    #[derive(serde::Deserialize, Default)]
                                    struct AuthOut {
                                        account_id: String,
                                        expires_at: i64,
                                    }
                                    match invoke::<AuthOut>("relay_authenticate", ()).await {
                                        Ok(a) => {
                                            let mins = ((a.expires_at - (js_sys::Date::now() / 1000.0) as i64)
                                                .max(0))
                                                / 60;
                                            flash(
                                                &ctx,
                                                format!("RELAY OK · session {mins}m · {}", a.account_id)
                                                    .as_str(),
                                            );
                                        }
                                        Err(e) => {
                                            let msg = if e.contains("no relay") {
                                                "NO RELAY URL SAVED".to_string()
                                            } else {
                                                format!("RELAY FAILED — {e}")
                                            };
                                            flash(&ctx, &msg);
                                        }
                                    }
                                    busy.set(false);
                                });
                            },
                            if *busy.read() { "Testing…" } else { "Test connection" }
                        }
                        button { class: "wl-btn-ghost",
                            onclick: move |_| {
                                let ctx = ctx;
                                spawn(async move {
                                    match invoke::<SyncStats>("sync_now", ()).await {
                                        Ok(s) => {
                                            record_sync(&ctx, &s);
                                            flash(&ctx, s.summary().as_str());
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
                    }
                }
            }

            // Page-level: one `settings_save` writes EVERY field above, so
            // the save action sits outside any section rather than reading
            // as belonging to Sync.
            div { class: "wl-page-actions",
                button { class: "wl-btn-primary", onclick: move |_| save(), "Save settings" }
                p { class: "wl-section-note", style: "margin: 2px 0 0; text-align: center;",
                    "Saves every field on this page."
                }
            }
            }
        }
    }
}
