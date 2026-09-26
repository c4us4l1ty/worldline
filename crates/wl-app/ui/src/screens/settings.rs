//! Settings — identity, appearance, intelligence, sync.
//!
//! API KEYS themselves live ONLY in the Stronghold vault — this screen
//! configures *which* provider, never the secret.
//!
//! Two persistence paths coexist here, on purpose:
//!
//! * The two appearance switches (theme, window pin) apply AND persist
//!   the moment they are touched. A theme toggle that only took effect
//!   on "Save settings" would look broken — you cannot evaluate a theme
//!   you are not allowed to see.
//! * Text fields (provider, model ids, relay URL) stage into a local
//!   draft and commit on "Save settings", which writes all of them.
//!
//! The switches persist from `ctx.settings` — the last state the SHELL
//! confirmed — with their own one field flipped, never from the local
//! draft. Sending the draft would silently commit whatever the user had
//! half-typed into an unrelated field. That rule is locked by
//! `switch_persists_from_shell_state_not_the_local_draft`.

use dioxus::prelude::*;

use crate::app::{
    flash, invoke, record_sync, AppCtx, AppSettingsView, IdentityStatus, Screen, SyncStats,
};
use crate::icons::{IconBack, IconMoon, IconSun};

/// Builds the payload an appearance switch persists.
///
/// Both switches (`theme`, `always_on_top`) write IMMEDIATELY, because a
/// theme you cannot preview is not a control. That makes the source of
/// the payload load-bearing: `shell` is the last state the shell
/// confirmed, `draft` is whatever the user has half-typed into the text
/// fields. Persisting the draft would silently commit an unrelated
/// in-progress edit — flipping the theme would save a half-typed relay
/// URL. So a switch always sends `shell` with its own one field flipped,
/// and the draft keeps its unsaved text untouched.
///
/// Extracted as a pure function so the rule is testable without a
/// browser; see `switch_persists_from_shell_state_not_the_local_draft`.
pub fn switch_payload(
    shell: &AppSettingsView,
    draft: &AppSettingsView,
    field: SwitchField,
) -> AppSettingsView {
    let mut base = shell.clone();
    match field {
        SwitchField::Theme => {
            // Toggles rather than sets, so the caller cannot pass the
            // value it already had and get a silent no-op.
            let next = if base.theme == "light" {
                "dark"
            } else {
                "light"
            };
            base.theme = next.to_string();
        }
        SwitchField::AlwaysOnTop => base.always_on_top = !shell.always_on_top,
    }
    // `draft` is deliberately unread: it exists in the signature so the
    // intent is checkable at the call site, and so a future edit that
    // starts reading it has to change this signature too.
    let _ = draft;
    base
}

/// Which preference an appearance switch owns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SwitchField {
    Theme,
    AlwaysOnTop,
}

pub fn SettingsScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut local = use_signal(|| ctx.settings.read().clone());
    let mut api_key = use_signal(String::new);
    let mut key_saved = use_signal(|| false);
    let mut busy = use_signal(|| false);
    // Separate guard for the relay handshake probe, so a connection test
    // in flight cannot be re-entered by the sync button beside it.
    let mut probing = use_signal(|| false);
    let mut phrase = use_signal(String::new);
    let mut identity_busy = use_signal(|| false);
    let mut identity_error = use_signal(String::new);
    let theme_controller = use_context::<crate::theme::Theme>();

    // The draft is seeded from context at mount and re-seeded from the
    // shell on every mount. Previously it was a mount-time snapshot that
    // never re-read the shell, so this page could display values the
    // store no longer held — and Save would then write those stale values
    // back over newer ones. The trade is deliberate: unsaved edits are
    // discarded when you navigate away and back, which is the honest
    // behaviour for a settings page and strictly better than silently
    // resurrecting a snapshot.
    {
        let mut local = local;
        use_effect(move || {
            spawn(async move {
                if let Ok(fresh) = invoke::<AppSettingsView>("settings_get", ()).await {
                    local.set(fresh);
                }
            });
        });
    }

    // Probe whether a key is already stored for the current provider.
    // Reads the draft inside the effect so provider switches re-probe;
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

    let save = move || {
        let mut ctx = ctx;
        let s = local.read().clone();
        spawn(async move {
            match invoke::<serde_json::Value>(
                "settings_save",
                serde_json::json!({ "settings": s.clone() }),
            )
            .await
            {
                Ok(_) => {
                    *ctx.settings.write() = s;
                    flash(&ctx, "SAVED");
                }
                Err(e) => flash(&ctx, &format!("ERR {e}")),
            }
        });
    };

    let s = local.read().clone();
    let status = ctx.identity_status.read().clone();
    let dark = s.theme != "light";
    let sync_label = crate::app::sync_label(&ctx);

    rsx! {
        div { class: "wl-page",
            // The way back used to be a button at the very bottom of a long
            // page, styled `.wl-btn-escape` — which reads as the bailout
            // affordance, not navigation. It is now a floating control in a
            // fixed header, the same place as every other page.
            div { class: "wl-page-head",
                button {
                    class: "wl-back",
                    aria_label: "Back to the line",
                    title: "Back to the line",
                    onclick: move |_| { { let mut s = ctx.screen; *s.write() = Screen::Canvas; } },
                    IconBack {}
                }
                div {
                    h1 { class: "wl-page-title", "Settings" }
                    p { class: "wl-page-sub",
                        "Secrets stay in the local vault. This screen only points the app at them."
                    }
                }
            }

            div { class: "wl-scroll-region",
            // ------------------------------------------------------------
            // Identity
            // ------------------------------------------------------------
            div { class: "wl-section",
                span { class: "wl-section-label", "Identity" }
                p { class: "wl-status",
                    // Coral is a beacon, never a surface (skill §1); an
                    // unlocked-identity dot is one of its three sanctioned
                    // uses (a cryptographic security badge).
                    if status.unlocked { span { class: "wl-status-dot wl-status-dot-sealed" } }
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
                            style: "margin-bottom: 10px;",
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
                    // The phrase input is a flex row rather than a bare
                    // field so "Restore" sits beside the words it acts on.
                    div { class: "wl-restore-row",
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
                            class: "wl-btn-ghost wl-btn-compact",
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
                        p { class: "wl-section-note", style: "margin-top: 8px; color: var(--wl-accent-coral);",
                            "{identity_error.read().clone()}"
                        }
                    }
                }
                p { class: "wl-section-note", style: "margin-top: 12px;",
                    "The phrase is never written to SQLite or the relay. Local directives work without it; sync and AI keys need it unlocked."
                }
            }

            // ------------------------------------------------------------
            // Appearance
            // ------------------------------------------------------------
            // Both controls here are switches rather than a `<select>` and
            // a label-bearing button. They apply instantly, because a theme
            // you cannot preview is not a control.
            div { class: "wl-section",
                span { class: "wl-section-label", "Appearance" }
                div { class: "wl-field",
                    label { class: "wl-label", "Theme" }
                    button {
                        class: "wl-switch",
                        role: "switch",
                        aria_checked: if dark { "true" } else { "false" },
                        aria_label: if dark {
                            "Theme: dark graphite. Switch to light parchment."
                        } else {
                            "Theme: light parchment. Switch to dark graphite."
                        },
                        onclick: move |_| {
                            let mut ctx = ctx;
                            // The draft carries the new theme for display;
                            // the payload is built from the shell state.
                            let next = if dark { "light" } else { "dark" };
                            theme_controller.set(next.to_string());
                            {
                                let mut v = local.read().clone();
                                v.theme = next.to_string();
                                local.set(v);
                            }
                            let base = switch_payload(
                                &ctx.settings.read(),
                                &local.peek(),
                                SwitchField::Theme,
                            );
                            spawn(async move {
                                match invoke::<serde_json::Value>(
                                    "settings_save",
                                    serde_json::json!({ "settings": base.clone() }),
                                )
                                .await
                                {
                                    Ok(_) => *ctx.settings.write() = base,
                                    Err(e) => flash(&ctx, &format!("ERR {e}")),
                                }
                            });
                        },
                        span { class: "wl-switch-label",
                            if dark { "Dark graphite" } else { "Light parchment" }
                        }
                        span { class: "wl-switch-track",
                            span { class: "wl-switch-knob",
                                if dark { IconMoon {} } else { IconSun {} }
                            }
                        }
                    }
                }
                div { class: "wl-field",
                    label { class: "wl-label", "Window" }
                    button {
                        class: "wl-switch",
                        role: "switch",
                        aria_checked: if s.always_on_top { "true" } else { "false" },
                        aria_label: "Pin window above other windows",
                        onclick: move |_| {
                            let pinned = !local.read().always_on_top;
                            let mut ctx = ctx;
                            // Optimistic flip, reverted below if either
                            // the window call or the persist fails: boot
                            // restores the persisted value, so an
                            // unpersisted pin would silently unpin on the
                            // next launch.
                            {
                                let mut cur = local;
                                let mut back = cur.read().clone();
                                back.always_on_top = pinned;
                                cur.set(back);
                            }
                            // Persist from the shell-confirmed state, not
                            // the draft — see `switch_payload`.
                            let base = switch_payload(
                                &ctx.settings.read(),
                                &local.peek(),
                                SwitchField::AlwaysOnTop,
                            );
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
                                    *ctx.settings.write() = base;
                                } else {
                                    let mut cur = local;
                                    let mut back = cur.read().clone();
                                    back.always_on_top = !pinned;
                                    cur.set(back);
                                    flash(&ctx, "PIN FAILED");
                                }
                            });
                        },
                        span { class: "wl-switch-label", "Pin above other windows" }
                        span { class: "wl-switch-track",
                            span { class: "wl-switch-knob",
                                span { class: "wl-switch-pip" }
                            }
                        }
                    }
                    p { class: "wl-section-note", style: "margin-top: 6px;",
                        "A pinned window floats above other apps — useful when the canvas should stay in view while you work."
                    }
                }
            }

            // ------------------------------------------------------------
            // Intelligence
            // ------------------------------------------------------------
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
                                class: "wl-btn-ghost wl-btn-compact wl-btn-danger",
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

            // ------------------------------------------------------------
            // Sync
            // ------------------------------------------------------------
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
                    button { class: "wl-btn-ghost",
                        disabled: *probing.read(),
                        onclick: move |_| {
                            // Uses the PERSISTED relay URL (the shell
                            // reads its own copy), so an unsaved edit
                            // must be saved before it can be tested.
                            let ctx = ctx;
                            probing.set(true);
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
                                probing.set(false);
                            });
                        },
                        if *probing.read() { "Testing relay…" } else { "Test connection" }
                    }
                }

                // The sync control itself, moved here from the canvas. It
                // gained the room to report what the last cycle did, which
                // the 34px circle never could.
                div { class: "wl-sync-panel",
                    button { class: "wl-sync-cta",
                        disabled: *busy.read(),
                        onclick: move |_| {
                            let ctx = ctx;
                            busy.set(true);
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
                                busy.set(false);
                            });
                        },
                        if *busy.read() { "Syncing…" } else { "Sync now" }
                    }
                    p { class: "wl-status wl-sync-meta",
                        // Honest about "never synced" rather than implying
                        // a freshness it has not got.
                        "{sync_label}"
                    }
                }
            }

            // Page-level: one `settings_save` writes EVERY text field
            // above, so the save action sits outside any section rather
            // than reading as belonging to one group. The two appearance
            // switches are the stated exception — they already persisted.
            div { class: "wl-page-actions",
                button { class: "wl-btn-primary", onclick: move |_| save(), "Save settings" }
                p { class: "wl-section-note", style: "margin: 2px 0 0; text-align: center;",
                    "Saves the provider, model ids and relay URL. Appearance switches save themselves."
                }
            }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> AppSettingsView {
        AppSettingsView {
            theme: "dark".into(),
            always_on_top: false,
            ai_provider: Some("openrouter".into()),
            tier1_model: Some("confirmed-architect".into()),
            tier2_model: Some("confirmed-dispatcher".into()),
            relay_url: Some("http://127.0.0.1:8080".into()),
        }
    }

    /// The load-bearing rule of the appearance switches. They persist on
    /// click, so if they ever read the draft instead of the shell state,
    /// flipping the theme silently commits whatever else the user had
    /// half-typed. Nothing about the UI stops that from regressing —
    /// it looks correct and is silent — so it is pinned here.
    #[test]
    fn switch_persists_from_shell_state_not_the_local_draft() {
        // A draft with unsaved edits in three unrelated fields.
        let draft = AppSettingsView {
            theme: "dark".into(),
            always_on_top: false,
            ai_provider: Some("qwen".into()),
            tier1_model: Some("half-typed-ar".into()),
            tier2_model: None,
            relay_url: Some("http://192.168.1.5:9999".into()),
        };

        let out = switch_payload(&shell(), &draft, SwitchField::Theme);

        // The switch's own field flipped...
        assert_eq!(out.theme, "light");
        // ...and every other field is the shell-confirmed value, NOT the
        // draft. This is the assertion that would fail on a regression.
        assert_eq!(out.ai_provider.as_deref(), Some("openrouter"));
        assert_eq!(out.tier1_model.as_deref(), Some("confirmed-architect"));
        assert_eq!(out.tier2_model.as_deref(), Some("confirmed-dispatcher"));
        assert_eq!(out.relay_url.as_deref(), Some("http://127.0.0.1:8080"));
    }

    /// The same rule for the pin switch, plus the flip direction.
    #[test]
    fn switch_payload_flips_only_its_own_field() {
        let shell = shell();
        let out = switch_payload(&shell, &shell, SwitchField::AlwaysOnTop);
        assert!(out.always_on_top);
        assert_eq!(out.theme, shell.theme);
        assert_eq!(out.relay_url, shell.relay_url);

        // Toggles back off from a pinned shell state.
        let pinned = AppSettingsView {
            always_on_top: true,
            ..shell.clone()
        };
        let out = switch_payload(&pinned, &pinned, SwitchField::AlwaysOnTop);
        assert!(!out.always_on_top);
    }

    /// Theme toggling must flip in both directions from any starting
    /// value, including an unrecognised one — otherwise a switch
    /// reading a corrupt `theme` row could get stuck on a value that is
    /// neither theme.
    #[test]
    fn theme_switch_toggles_from_any_starting_value() {
        // Anything that is not exactly "light" is treated as dark (the
        // same normalization the shell and `Theme::set` apply), so it must
        // toggle to light…
        for start in ["dark", "Dark", "", "neon", "LIGHT"] {
            let s = AppSettingsView {
                theme: start.into(),
                ..shell()
            };
            let out = switch_payload(&s, &s, SwitchField::Theme);
            assert_eq!(out.theme, "light", "start {start:?} must toggle to light");
            // …and toggling back must land on dark.
            let back = switch_payload(&out, &out, SwitchField::Theme);
            assert_eq!(back.theme, "dark", "toggling back must land on dark");
        }
        // "light" is the one start that toggles the other way.
        let lit = AppSettingsView {
            theme: "light".into(),
            ..shell()
        };
        assert_eq!(switch_payload(&lit, &lit, SwitchField::Theme).theme, "dark");
    }
}
