//! BYOK Engine Initialization (spec Screen 2, FSM BYOK_KEY_STORE).
//!
//! Rendered once after seed verification. Manual skip is allowed
//! (PRD delta 6 — Tier-3 local engine works with no API key).
//! Secrets are write-only: the input clears after sealing and the key
//! itself is never rendered.

use dioxus::prelude::*;

use crate::app::{flash, invoke, AppCtx, Screen};

const PROVIDERS: [(&str, &str); 3] = [
    ("anthropic", "Anthropic"),
    ("openai-compat", "OpenAI"),
    ("openrouter", "OpenRouter"),
];

pub fn ByokSetupScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut provider = use_signal(|| {
        ctx.settings
            .read()
            .ai_provider
            .clone()
            .unwrap_or_else(|| "anthropic".into())
    });
    let mut api_key = use_signal(String::new);
    let mut key_saved = use_signal(|| false);
    let mut busy = use_signal(|| false);

    // Probe vault presence for the selected provider. Subscribes to the
    // provider signal so pill switches re-probe; the in-flight guard
    // drops stale responses from rapid switches.
    use_effect(move || {
        let p = provider.read().clone();
        let mut saved = key_saved;
        let current = provider;
        spawn(async move {
            let has: bool = invoke("has_api_key", serde_json::json!({ "provider": p.clone() }))
                .await
                .unwrap_or(false);
            if *current.peek() == p {
                saved.set(has);
            }
        });
    });

    let s = ctx.settings.read().clone();
    let tier1 = s
        .tier1_model
        .clone()
        .unwrap_or_else(|| "your Tier-1 pick".into());
    let tier2 = s
        .tier2_model
        .clone()
        .unwrap_or_else(|| "your Tier-2 pick".into());

    rsx! {
        div { class: "wl-scroll-region",
            div { class: "wl-hud", style: "margin-bottom: 14px;",
                span { class: "wl-hud-pill", "Vault: sealed" }
                span { class: "wl-hud-status",
                    span { class: "wl-pulse-dot" }
                    "Stronghold"
                }
            }
            h1 { class: "wl-serif-title",
                "Execution " span { class: "wl-italic-accent", "intelligence" }
            }
            p { class: "wl-body-muted", style: "margin: 10px 0 18px;",
                "Worldline delegates planning to your own frontier keys. Credentials pass directly into OS Stronghold isolation."
            }

            div { class: "wl-field",
                label { class: "wl-label", "Active provider" }
                div { class: "wl-provider-pills",
                    for (id, label) in PROVIDERS {
                        button {
                            class: if *provider.read() == id { "wl-provider-pill wl-provider-pill-active" } else { "wl-provider-pill" },
                            onclick: move |_| provider.set(id.to_string()),
                            "{label}"
                        }
                    }
                }
            }

            div { class: "wl-field",
                label { class: "wl-label", "Provider API key" }
                input {
                    class: "wl-input wl-mono",
                    r#type: "password",
                    placeholder: "sk-ant-… (sealed, never displayed again)",
                    autocomplete: "off",
                    spellcheck: "false",
                    value: "{api_key.read().clone()}",
                    oninput: move |e| api_key.set(e.value()),
                }
                p { class: "wl-seed-sub", style: "margin-top: 6px;",
                    "Injected directly into the Stronghold vault. Never written to SQLite, the DOM, or the relay."
                }
                div { style: "display: flex; gap: 8px; margin-top: 8px; align-items: center;",
                    button {
                        class: "wl-btn-ghost",
                        disabled: *busy.read(),
                        onclick: move |_| {
                            let ctx = ctx;
                            let p = provider.read().clone();
                            let key = api_key.read().clone();
                            if key.trim().is_empty() {
                                flash(&ctx, "KEY EMPTY");
                                return;
                            }
                            busy.set(true);
                            spawn(async move {
                                match invoke::<bool>(
                                    "set_api_key",
                                    serde_json::json!({ "provider": p, "key": key }),
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
                                busy.set(false);
                            });
                        },
                        if *key_saved.read() { "Replace vault key" } else { "Save key to vault" }
                    }
                    if *key_saved.read() {
                        span { class: "wl-hud-pill", "sealed" }
                    }
                }
            }

            div { class: "wl-directive-card", style: "margin: 6px 0 14px; padding: 18px;",
                div { class: "wl-directive-step-badge", "Model dispatch allocation" }
                p { class: "wl-body-muted", "Tier 1 (Architect): {tier1}" }
                p { class: "wl-body-muted", "Tier 2 (Dispatcher): {tier2}" }
                p { class: "wl-body-muted", "Tier 3 (Local Core): Rust Native Deterministic Engine" }
            }

            button {
                class: "wl-btn-primary",
                onclick: move |_| {
                    // Persist the provider choice into settings (wrapped shape:
                    // shell param is named `settings`).
                    let ctx = ctx;
                    let mut next = ctx.settings.read().clone();
                    next.ai_provider = Some(provider.read().clone());
                    spawn(async move {
                        let _ : Result<serde_json::Value, String> = invoke(
                            "settings_save",
                            serde_json::json!({ "settings": next.clone() }),
                        )
                        .await;
                        {
                            let mut sig = ctx.settings;
                            *sig.write() = next;
                        }
                        { let mut s = ctx.screen; *s.write() = Screen::MorningBrief; }
                    });
                },
                "Initialize Sovereign Workspace"
            }
            button {
                class: "wl-btn-escape",
                style: "margin-top: 8px;",
                onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::MorningBrief; } },
                "Continue without key — manual mode"
            }
        }
    }
}
