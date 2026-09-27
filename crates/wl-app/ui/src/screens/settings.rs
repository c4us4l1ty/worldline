//! Settings — identity, appearance, intelligence, sync.
//!
//! API KEYS themselves live ONLY in the Stronghold vault — this screen
//! configures *which* provider, never the secret.
//!
//! # One save path (2026-09-27)
//!
//! This page used to have two, and said so on screen: the two appearance
//! switches applied and persisted on click, everything else staged into a
//! draft and committed by a "Save settings" button, with a note under the
//! button explaining the exception. That note was an admission that the
//! model was wrong.
//!
//! Now every field commits on `change` — blur or Enter, never on `input`,
//! so a half-typed relay URL is never written — and the button and the
//! note are gone. One path, and nothing on the page describes it.
//!
//! # What a commit sends
//!
//! `settings_save` writes **every** field, so a commit cannot be a patch:
//! it has to send whole state. Sending the whole *draft* would commit
//! whatever else the user had in progress, and the original reason the
//! theme switch built its own payload was to avoid exactly that. The rule
//! generalised into [`commit_payload`]: take the last state the shell
//! confirmed, and apply **only** the field the user just changed. Flipping
//! the theme can no longer save a half-typed relay URL, and picking a
//! model can no longer save a half-typed API key. The behaviour is one
//! function and it is tested.
//!
//! A failed commit reverts the draft to the last confirmed state. It used
//! to flash `ERR …` and keep showing the unsaved value, which meant a
//! locked vault or a full disk left the page displaying settings that were
//! not in the database.

use dioxus::prelude::*;

use crate::app::{
    flash, invoke, record_sync, AppCtx, AppSettingsView, IdentityStatus, ModelListView, Screen,
    SyncStats,
};
use crate::icons::{IconBack, IconChevron, IconMoon, IconSun, IconSync};
use crate::screens::model_picker::{
    chosen_meta, provider_label, ModelPicker, ProviderPicker, Tier,
};

/// Which preference a commit is for.
///
/// A commit is scoped to one field because `settings_save` overwrites the
/// whole row. The enum is what makes that scoping checkable: the caller
/// names the field, and [`commit_payload`] is the only place that decides
/// what that name is allowed to carry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingsField {
    Theme,
    Provider,
    ArchitectModel,
    DispatcherModel,
    RelayUrl,
}

/// Builds the payload a single field's commit sends.
///
/// `confirmed` is the last state the shell acknowledged; `draft` is
/// whatever is on screen. The result is `confirmed` with **only** `field`
/// taken from the draft.
///
/// `Provider` is the one field that carries more than itself, and that is
/// not an inconsistency: choosing a provider invalidates both model slots,
/// because a model id is provider-specific. Sending an OpenRouter id to
/// Google produces a 404 the user has no way to interpret, so the two ids
/// are part of the same edit and are committed with it. `with_provider`
/// applies the same rule to the on-screen draft, so what is shown and what
/// is stored cannot disagree.
pub fn commit_payload(
    confirmed: &AppSettingsView,
    draft: &AppSettingsView,
    field: SettingsField,
) -> AppSettingsView {
    let mut base = confirmed.clone();
    match field {
        SettingsField::Theme => base.theme = draft.theme.clone(),
        SettingsField::Provider => {
            base.ai_provider = draft.ai_provider.clone();
            base.tier1_model = draft.tier1_model.clone();
            base.tier2_model = draft.tier2_model.clone();
        }
        SettingsField::ArchitectModel => base.tier1_model = draft.tier1_model.clone(),
        SettingsField::DispatcherModel => base.tier2_model = draft.tier2_model.clone(),
        SettingsField::RelayUrl => base.relay_url = draft.relay_url.clone(),
    }
    base
}

/// Applies a provider change to a draft: sets the provider and clears both
/// model slots.
///
/// Both start unset, by design — nothing is ever chosen for you, so a
/// provider change cannot leave a stale id pointing at a different
/// provider's catalog.
pub fn with_provider(mut s: AppSettingsView, id: String) -> AppSettingsView {
    s.ai_provider = if id.trim().is_empty() { None } else { Some(id) };
    s.tier1_model = None;
    s.tier2_model = None;
    s
}

/// The theme a switch moves to from `current`.
///
/// Anything that is not exactly `"light"` is dark, which is the same
/// normalization the shell and `Theme::set` apply. A switch reading a
/// corrupt or absent `theme` row must still be able to move, or it is
/// stuck on a value that is neither theme.
pub fn next_theme(current: &str) -> &'static str {
    if current == "light" {
        "dark"
    } else {
        "light"
    }
}

/// Sends one commit and reconciles the two signals around it.
///
/// `confirmed` advances only on the shell's answer, and the draft reverts
/// on a failure. Keeping the confirmed state in its own signal is what
/// lets a commit be scoped to a single field: without it there would be no
/// baseline to scope against, and the only alternative is sending the whole
/// draft — which is the bug this replaced.
///
/// A plain function taking the three `Copy` handles rather than a closure
/// over them, so the call sites in the markup read as a commit rather than
/// as a captured variable.
fn commit(
    mut ctx: AppCtx,
    local: Signal<AppSettingsView>,
    mut confirmed: Signal<AppSettingsView>,
    payload: AppSettingsView,
) {
    spawn(async move {
        match invoke::<serde_json::Value>(
            "settings_save",
            serde_json::json!({ "settings": payload.clone() }),
        )
        .await
        {
            Ok(_) => {
                *ctx.settings.write() = payload.clone();
                confirmed.set(payload);
            }
            Err(e) => {
                // Put the page back in step with the store. Showing a value
                // the database does not hold is the specific failure this
                // replaces: the old code flashed an error and left the
                // unsaved draft on screen.
                let mut draft = local;
                draft.set(confirmed.peek().clone());
                flash(&ctx, &format!("NOT SAVED — {e}"));
            }
        }
    });
}

/// An empty text field means "not set", not `Some("")`.
///
/// The store's shape-exact validation rejects an empty relay URL rather
/// than coercing it, so sending one would fail the write and — under the
/// old commit model — surface as a save error the user could not
/// interpret.
pub fn optional(value: String) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value)
    }
}

pub fn SettingsScreen() -> Element {
    let ctx = use_context::<AppCtx>();
    let mut local = use_signal(|| ctx.settings.read().clone());
    // The last state the SHELL confirmed. Every commit is scoped to this
    // rather than to the draft, and a failed commit reverts the draft to
    // it — see the module docs.
    let confirmed = use_signal(|| ctx.settings.read().clone());
    let mut api_key = use_signal(String::new);
    let mut key_saved = use_signal(|| false);
    let mut busy = use_signal(|| false);
    // Separate guard for the relay handshake probe, so a connection test
    // in flight cannot be re-entered by the sync button beside it.
    let mut probing = use_signal(|| false);
    let mut phrase = use_signal(String::new);
    let mut identity_busy = use_signal(|| false);
    let mut identity_error = use_signal(String::new);
    // Model catalog for the selected provider, fetched from the provider
    // itself and never from a bundled list. `None` means "not loaded yet"
    // as distinct from "loaded and empty" — the picker says something
    // different for each.
    let mut catalog = use_signal(|| None::<ModelListView>);
    let mut catalog_error = use_signal(|| None::<String>);
    let mut catalog_busy = use_signal(|| false);
    let mut open_picker = use_signal(|| None::<ChoiceSheet>);
    let theme_controller = use_context::<crate::theme::Theme>();

    // The draft is seeded from the shell on every mount. It used to be a
    // mount-time snapshot that never re-read the shell, so this page could
    // display values the store no longer held — and a commit would then
    // write those stale values back over newer ones. Unsaved edits are
    // discarded on navigate-away-and-back, which is the honest behaviour
    // for a settings page and strictly better than resurrecting a
    // snapshot.
    {
        let mut local = local;
        let mut confirmed = confirmed;
        use_effect(move || {
            spawn(async move {
                if let Ok(fresh) = invoke::<AppSettingsView>("settings_get", ()).await {
                    confirmed.set(fresh.clone());
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

    // Load the provider's model list once a key exists. Same
    // stale-response discipline as the probe above: switching provider
    // mid-flight must not leave the previous provider's models on screen.
    //
    // `force` is what makes a newly released model reachable without
    // waiting out the shell's one-hour cache.
    let mut refresh_catalog = move |force: bool| {
        let provider = local.read().ai_provider.clone().unwrap_or_default();
        if provider.is_empty() || !*key_saved.read() {
            *catalog.write() = None;
            catalog_error.set(None);
            return;
        }
        let still_current = local;
        catalog_busy.set(true);
        catalog_error.set(None);
        spawn(async move {
            match invoke::<ModelListView>(
                "list_models",
                serde_json::json!({ "provider": provider.clone(), "force": force }),
            )
            .await
            {
                Ok(list) => {
                    if still_current.peek().ai_provider.clone().unwrap_or_default() == provider {
                        *catalog.write() = Some(list);
                    }
                }
                Err(e) => {
                    if still_current.peek().ai_provider.clone().unwrap_or_default() == provider {
                        *catalog.write() = None;
                        catalog_error.set(Some(e));
                    }
                }
            }
            catalog_busy.set(false);
        });
    };
    {
        let mut refresh = refresh_catalog;
        use_effect(move || {
            // Re-runs when the provider or the key-presence flips, which
            // is exactly when a catalog becomes fetchable.
            let _ = local.read().ai_provider.clone();
            let _ = key_saved.read();
            refresh(false);
        });
    }

    // `edit` reads as a field edit at the call site and exists so the
    // read-draft / apply / scope / commit sequence is written once. A plain
    // function rather than a closure: a closure would have to be declared
    // `mut` at three call sites, and the `Box<dyn FnOnce>` argument means it
    // is `Fn`, not `FnMut` — so `mut` there would be a lie.
    fn edit(
        ctx: AppCtx,
        mut local: Signal<AppSettingsView>,
        confirmed: Signal<AppSettingsView>,
        field: SettingsField,
        apply: Box<dyn FnOnce(&mut AppSettingsView)>,
    ) {
        let mut draft = local.read().clone();
        apply(&mut draft);
        local.set(draft);
        commit(
            ctx,
            local,
            confirmed,
            commit_payload(&confirmed.peek(), &local.peek(), field),
        );
    }

    let s = local.read().clone();
    let status = ctx.identity_status.read().clone();
    let dark = s.theme != "light";
    let sync_label = crate::app::sync_label(&ctx);
    let provider = s.ai_provider.clone().unwrap_or_default();
    let known_models: Vec<crate::app::ModelInfoView> = catalog
        .read()
        .as_ref()
        .map(|c| c.models.clone())
        .unwrap_or_default();
    // The two tier rows, resolved before the rsx because Dioxus has no
    // `let` inside an element body. The meta line is the model's context
    // window, and only when the catalog can state it — absent rather than
    // a placeholder, because the row's third line is a fact or it is not
    // there.
    let slots: Vec<(Tier, Option<String>, Option<String>)> = [
        (Tier::Architect, &s.tier1_model),
        (Tier::Dispatcher, &s.tier2_model),
    ]
    .into_iter()
    .map(|(tier, id)| {
        let meta = id.as_deref().and_then(|i| chosen_meta(&known_models, i));
        (tier, id.clone(), meta)
    })
    .collect();

    rsx! {
        div { class: "wl-page",
            // The way back used to be a button at the very bottom of a long
            // page, styled `.wl-btn-escape` — which reads as the bailout
            // affordance, not navigation. It is a floating control in a
            // fixed header, the same place as every other page.
            div { class: "wl-page-head",
                button {
                    class: "wl-circle-btn wl-back",
                    aria_label: "Back to the line",
                    title: "Back to the line",
                    onclick: move |_| { { let mut sc = ctx.screen; *sc.write() = Screen::Canvas; } },
                    IconBack {}
                }
                h1 { class: "wl-page-title", "Settings" }
            }

            div { class: "wl-scroll-region",
            // ------------------------------------------------------------
            // Identity
            // ------------------------------------------------------------
            section { class: "wl-section",
                h2 { class: "wl-section-title", "Identity" }
                div { class: "wl-fieldset",
                    p { class: "wl-status",
                        // Coral is a beacon, never a surface (skill §1); an
                        // unlocked-identity dot is one of its three
                        // sanctioned uses (a cryptographic security badge).
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
                        div { style: "margin-top: 14px;",
                            button {
                                class: "wl-btn-ghost",
                                disabled: *identity_busy.read(),
                                onclick: move |_| {
                                    {
                                        let mut sc = ctx.screen;
                                        *sc.write() = Screen::SeedVault {
                                            phrase: Vec::new(),
                                            verify_indices: Vec::new(),
                                            restore: false,
                                            has_identity: false,
                                        };
                                    };
                                },
                                "Generate a new identity"
                            }
                        }
                    } else if !status.unlocked {
                        if status.vault_has_mnemonic {
                            div { style: "margin-top: 14px;",
                                button {
                                    class: "wl-btn-ghost",
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
                        }
                        // The phrase input is a flex row rather than a bare
                        // field so "Restore" sits beside the words it acts on.
                        div { class: "wl-restore-row", style: "margin-top: 10px;",
                            input {
                                class: "wl-input wl-input--mono",
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
                            p { class: "wl-form-error", "{identity_error.read().clone()}" }
                        }
                    }
                    p { class: "wl-hint",
                        "The phrase is never written to the database or the relay. Local directives work without it; sync and AI keys need it unlocked."
                    }
                }
            }

            // ------------------------------------------------------------
            // Appearance
            // ------------------------------------------------------------
            // The theme switch. The pin switch that used to sit here is
            // gone with the feature — see PRD delta 175.
            section { class: "wl-section",
                h2 { class: "wl-section-title", "Appearance" }
                div { class: "wl-fieldset",
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
                            let next = next_theme(&local.peek().theme);
                            // Applied first and unconditionally: a switch
                            // that only took effect on save would appear
                            // broken, because you cannot evaluate a theme
                            // you are not allowed to see.
                            theme_controller.set(next.to_string());
                            edit(ctx, local, confirmed, SettingsField::Theme, Box::new(move |d| d.theme = next.to_string()));
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
            }

            // ------------------------------------------------------------
            // Intelligence
            // ------------------------------------------------------------
            // Three choice rows, one component, one open gesture. The
            // provider was a native `<select>` until 2026-09-27 and shipped
            // twice as a near-white field carrying linen text inside this
            // graphite card.
            section { class: "wl-section",
                h2 { class: "wl-section-title", "Intelligence" }

                div { class: "wl-fieldset",
                    button {
                        class: "wl-value-row",
                        onclick: move |_| *open_picker.write() = Some(ChoiceSheet::Provider),
                        span { class: "wl-value-row-label", "AI provider" }
                        if provider.is_empty() {
                            span { class: "wl-value-row-value wl-value-row-value--empty",
                                "None — manual mode"
                            }
                        } else {
                            span { class: "wl-value-row-value", "{provider_label(&provider)}" }
                        }
                        span { class: "wl-value-row-go", IconChevron {} }
                    }
                    p { class: "wl-hint",
                        "Manual mode plans goals from your own words and makes no API calls. Choosing a provider lets the architect name the goal and build the milestone tree."
                    }
                }

                // Tier slots. A row that reads as a value with a chevron,
                // not an input: the point is that these are chosen from a
                // list, not typed. The catalog comes from the provider's
                // LIVE list, so a model released upstream is selectable
                // without a Worldline release.
                for (tier, chosen, meta) in slots {
                    div { key: "{tier.label()}", class: "wl-fieldset",
                        button {
                            class: "wl-value-row",
                            onclick: {
                                let t = tier;
                                move |_| {
                                    catalog_error.set(None);
                                    // Fetch only if we have nothing. This
                                    // used to null the catalog on every
                                    // open, on the reasoning that the
                                    // sheet should show a loading state —
                                    // but nothing refetched it, because the
                                    // load effect only reacts to a
                                    // provider or key change. So the sheet
                                    // opened empty and reported "no models"
                                    // while the page behind it said "443
                                    // models loaded".
                                    if catalog.peek().is_none() {
                                        refresh_catalog(false);
                                    }
                                    *open_picker.write() = Some(ChoiceSheet::Tier(t));
                                }
                            },
                            span { class: "wl-value-row-label", "{tier.label()}" }
                            if let Some(id) = &chosen {
                                span { class: "wl-value-row-value", "{id}" }
                            } else {
                                span { class: "wl-value-row-value wl-value-row-value--empty",
                                    "Not selected"
                                }
                            }
                            if let Some(m) = &meta {
                                span { class: "wl-value-row-meta", "{m} context" }
                            }
                            span { class: "wl-value-row-go", IconChevron {} }
                        }
                    }
                }

                // Catalog status. The list always comes from the provider,
                // never from this build — so the refresh is how a model
                // released upstream is picked up without waiting out the
                // shell's cache.
                div { class: "wl-fieldset",
                    div { class: "wl-catalog-status",
                        if *catalog_busy.read() {
                            span { class: "wl-mono", "loading models…" }
                        } else if let Some(err) = catalog_error.read().clone() {
                            // Rendered inline, and in full: this is the
                            // message that used to be discarded.
                            span { class: "wl-form-error", "{err}" }
                        } else if let Some(list) = catalog.read().clone() {
                            span { class: "wl-mono",
                                "{list.models.len()} models"
                                if list.cached { " · cached" }
                            }
                            if let Some(total) = list.truncated_from {
                                span { class: "wl-hint", style: "margin: 0;",
                                    "showing the first {total} — refine with search"
                                }
                            }
                        } else if s.ai_provider.is_some() && *key_saved.read() {
                            span { class: "wl-hint", style: "margin: 0;", "no models loaded" }
                        }
                        if s.ai_provider.is_some() && *key_saved.read() {
                            button {
                                class: "wl-btn-ghost wl-btn-compact",
                                disabled: *catalog_busy.read(),
                                onclick: move |_| refresh_catalog(true),
                                "Refresh models"
                            }
                        }
                    }
                }

                div { class: "wl-fieldset",
                    span { class: "wl-fieldset-label", "API key" }
                    input { class: "wl-input wl-input--mono", r#type: "password",
                        placeholder: "Sealed into this device's vault",
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
                                            // A new key may unlock a
                                            // catalog that was refused
                                            // before, so refetch rather
                                            // than leaving the picker
                                            // stuck on an auth error.
                                            *catalog.write() = None;
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
                                                *catalog.write() = None;
                                            }
                                            _ => flash(&ctx, "KEY NOT FOUND"),
                                        }
                                    });
                                },
                                "Remove"
                            }
                        }
                    }
                    p { class: "wl-hint",
                        "Sealed into the local Stronghold vault, then cleared from this field. Never written to the database, never sent to the relay."
                    }
                }
            }

            // ------------------------------------------------------------
            // Sync
            // ------------------------------------------------------------
            section { class: "wl-section",
                h2 { class: "wl-section-title", "Sync" }
                div { class: "wl-fieldset",
                    span { class: "wl-fieldset-label", "Relay URL" }
                    input { class: "wl-input wl-input--mono", r#type: "text",
                        placeholder: "http://127.0.0.1:8080",
                        value: "{s.relay_url.clone().unwrap_or_default()}",
                        oninput: move |e| {
                            let v = e.value();
                            let mut draft = local.read().clone();
                            draft.relay_url = optional(v);
                            local.set(draft);
                        },
                        // `onchange` fires on blur or Enter, never per
                        // keystroke — so an abandoned half-typed host is
                        // never committed.
                        onchange: move |e| {
                            let v = optional(e.value());
                            edit(ctx, local, confirmed, SettingsField::RelayUrl, Box::new(move |d| d.relay_url = v));
                        } }
                    div { class: "wl-inline-actions",
                        button {
                            class: "wl-btn-ghost wl-btn-compact",
                            disabled: *probing.read(),
                            onclick: move |_| {
                                // Uses the PERSISTED relay URL (the shell
                                // reads its own copy), so an unsaved edit
                                // must have been committed before it can be
                                // tested. Under persist-on-change that has
                                // already happened by the time focus left
                                // the field.
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
                }

                div { class: "wl-fieldset",
                    // The sync control itself, moved here from the canvas.
                    // It gained the room to report what the last cycle
                    // did, which the 34px circle never could.
                    button { class: "wl-sync-cta",
                        disabled: *busy.read(),
                        onclick: move |_| {
                            let ctx = ctx;
                            busy.set(true);
                            spawn(async move {
                                match invoke::<SyncStats>("sync_now", ()).await {
                                    Ok(stats) => {
                                        record_sync(&ctx, &stats);
                                        flash(&ctx, stats.summary().as_str());
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
                        span { IconSync {} }
                        span { if *busy.read() { "Syncing…" } else { "Sync now" } }
                    }
                    p { class: "wl-status wl-sync-meta",
                        // Honest about "never synced" rather than implying
                        // a freshness it has not got.
                        "{sync_label}"
                    }
                }
            }
            }
        }

        // The choice sheets, rendered over this page rather than as global
        // overlays like the nav and telemetry drawers: they are only ever
        // opened from here, so they can own local signals instead of
        // growing `AppCtx` for a transient concern.
        if let Some(sheet) = *open_picker.read() {
            match sheet {
                ChoiceSheet::Provider => rsx! {
                    ProviderPicker {
                        current: provider.clone(),
                        on_pick: move |id: String| {
                            // A provider change invalidates the catalog we
                            // were holding, so drop it and let the load
                            // effect refetch for the new provider.
                            *catalog.write() = None;
                            catalog_error.set(None);
                            let draft = with_provider(local.read().clone(), id);
                            local.set(draft);
                            commit(
                                ctx,
                                local,
                                confirmed,
                                commit_payload(
                                    &confirmed.peek(),
                                    &local.peek(),
                                    SettingsField::Provider,
                                ),
                            );
                            *open_picker.write() = None;
                        },
                        on_close: move |_| {
                            *open_picker.write() = None;
                        },
                    }
                },
                ChoiceSheet::Tier(tier) => rsx! {
                    ModelPicker {
                        tier,
                        provider: provider.clone(),
                        models: known_models.clone(),
                        recommended: catalog
                            .read()
                            .as_ref()
                            .map(|c| c.recommended.clone())
                            .unwrap_or_default(),
                        cached: catalog.read().as_ref().is_some_and(|c| c.cached),
                        loading: *catalog_busy.read(),
                        error: catalog_error.read().clone(),
                        on_pick: move |id: String| {
                            let field = match tier {
                                Tier::Architect => SettingsField::ArchitectModel,
                                Tier::Dispatcher => SettingsField::DispatcherModel,
                            };
                            edit(ctx, local, confirmed, field, Box::new(move |d| match tier {
                                Tier::Architect => d.tier1_model = Some(id),
                                Tier::Dispatcher => d.tier2_model = Some(id),
                            }));
                            *open_picker.write() = None;
                        },
                        on_close: move |_| {
                            *open_picker.write() = None;
                        },
                    }
                },
            }
        }
    }
}

/// Which choice sheet is open, if any.
///
/// One signal rather than two: two independent `Option`s could both be
/// `Some`, and the markup would then render two sheets with the second one
/// unreachable behind the first.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ChoiceSheet {
    Provider,
    Tier(Tier),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn confirmed() -> AppSettingsView {
        AppSettingsView {
            theme: "dark".into(),
            ai_provider: Some("openrouter".into()),
            tier1_model: Some("confirmed-architect".into()),
            tier2_model: Some("confirmed-dispatcher".into()),
            relay_url: Some("http://127.0.0.1:8080".into()),
        }
    }

    /// A draft with in-progress edits in three unrelated fields.
    fn dirty_draft() -> AppSettingsView {
        AppSettingsView {
            theme: "light".into(),
            ai_provider: Some("google".into()),
            tier1_model: Some("half-typed-ar".into()),
            tier2_model: Some("also-uncommitted".into()),
            relay_url: Some("http://192.168.1.5:9999".into()),
        }
    }

    /// The load-bearing rule of the commit path, and the one that is
    /// impossible to see in the UI if it regresses: every field looks
    /// correct, the value you changed saves, and the damage lands silently
    /// on a field you were not looking at.
    ///
    /// `settings_save` overwrites the whole row, so a commit must not send
    /// the whole draft — it sends the last confirmed state with one field
    /// applied. This is what replaced the theme switch's bespoke payload,
    /// which existed for exactly this reason and was the only field that
    /// had it.
    #[test]
    fn a_commit_sends_one_field_and_never_the_rest_of_the_draft() {
        let out = commit_payload(&confirmed(), &dirty_draft(), SettingsField::Theme);

        // The field the user touched...
        assert_eq!(out.theme, "light");
        // ...and nothing else. These are the assertions that would fail on
        // a regression: the relay URL and both model ids are mid-edit and
        // must not be written.
        assert_eq!(out.relay_url.as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(out.ai_provider.as_deref(), Some("openrouter"));
        assert_eq!(out.tier1_model.as_deref(), Some("confirmed-architect"));
        assert_eq!(out.tier2_model.as_deref(), Some("confirmed-dispatcher"));
    }

    /// The same rule for every remaining field, so a new field cannot be
    /// added with a wider scope than its neighbours.
    #[test]
    fn every_field_scopes_its_own_commit_and_nothing_else() {
        // The theme is never carried by any commit but its own. The draft
        // has it flipped as a side effect of the fixture, so it is the
        // sharpest probe: it is the value a user is LEAST likely to be
        // editing when they touch something else, and the one whose
        // unauthorized commit is most visible — the whole page re-themes.
        for field in [
            SettingsField::Provider,
            SettingsField::ArchitectModel,
            SettingsField::DispatcherModel,
            SettingsField::RelayUrl,
        ] {
            let out = commit_payload(&confirmed(), &dirty_draft(), field);
            assert_eq!(
                out.theme, "dark",
                "{field:?} leaked an unrelated theme change"
            );
        }

        let architect = commit_payload(&confirmed(), &dirty_draft(), SettingsField::ArchitectModel);
        assert_eq!(architect.tier1_model.as_deref(), Some("half-typed-ar"));
        assert_eq!(
            architect.tier2_model.as_deref(),
            Some("confirmed-dispatcher"),
            "the other slot must not be committed by this field"
        );

        let dispatcher =
            commit_payload(&confirmed(), &dirty_draft(), SettingsField::DispatcherModel);
        assert_eq!(dispatcher.tier2_model.as_deref(), Some("also-uncommitted"));
        assert_eq!(
            dispatcher.tier1_model.as_deref(),
            Some("confirmed-architect")
        );

        let relay = commit_payload(&confirmed(), &dirty_draft(), SettingsField::RelayUrl);
        // Its own value gets through…
        assert_eq!(relay.relay_url.as_deref(), Some("http://192.168.1.5:9999"));
        // …and nothing else does. The scoping rule is about what a commit
        // must NOT carry, so it is worth stating the positive case too: a
        // commit that carried nothing would satisfy every leak assertion
        // above while silently discarding every edit.
        assert_eq!(relay.tier1_model.as_deref(), Some("confirmed-architect"));
        assert_eq!(relay.tier2_model.as_deref(), Some("confirmed-dispatcher"));
        assert_eq!(relay.ai_provider.as_deref(), Some("openrouter"));
    }

    /// Choosing a provider is one edit with a defined consequence: both
    /// model ids belong to the old provider's catalog and are invalid
    /// against the new one. So the provider field carries three values.
    ///
    /// This is the one place a commit is wider than its name, and it has to
    /// be — but it has to be *exactly* three, or the scoping rule above
    /// has an exception nobody can see.
    #[test]
    fn a_provider_change_commits_its_two_stale_model_ids_as_well() {
        let out = commit_payload(&confirmed(), &dirty_draft(), SettingsField::Provider);
        assert_eq!(out.ai_provider.as_deref(), Some("google"));
        assert_eq!(out.tier1_model.as_deref(), Some("half-typed-ar"));
        assert_eq!(out.tier2_model.as_deref(), Some("also-uncommitted"));
        // And still not the theme or the relay.
        assert_eq!(out.theme, "dark");
        assert_eq!(out.relay_url.as_deref(), Some("http://127.0.0.1:8080"));
    }

    /// The draft and the commit have to agree about the invalidation, or
    /// the rows would clear on screen while the old ids stayed in the
    /// database — the exact state that produces an unexplainable 404.
    #[test]
    fn the_draft_clears_both_slots_so_shown_and_stored_agree() {
        let swapped = with_provider(confirmed(), "google".into());
        assert_eq!(swapped.ai_provider.as_deref(), Some("google"));
        assert_eq!(swapped.tier1_model, None);
        assert_eq!(swapped.tier2_model, None);
        // Everything else is untouched.
        assert_eq!(swapped.theme, "dark");
        assert_eq!(swapped.relay_url.as_deref(), Some("http://127.0.0.1:8080"));

        // And the manual-mode entry, which is an empty id, is `None`
        // rather than `Some("")` — the store's shape-exact validation
        // rejects an empty provider string.
        let manual = with_provider(confirmed(), String::new());
        assert_eq!(manual.ai_provider, None);
        assert_eq!(manual.tier1_model, None);
    }

    /// A provider id that is whitespace is manual mode too, not a provider
    /// whose name is a space.
    #[test]
    fn a_blank_provider_is_manual_mode() {
        let s = with_provider(confirmed(), "   ".into());
        assert_eq!(s.ai_provider, None);
    }

    /// The switch must move from any starting value, including a corrupt
    /// or absent `theme` row, or it can get stuck on a value that is
    /// neither theme.
    #[test]
    fn the_theme_switch_toggles_from_any_starting_value() {
        // Anything that is not exactly "light" is dark — the same
        // normalization the shell and `Theme::set` apply — so it must
        // toggle to light…
        for start in ["dark", "Dark", "", "neon", "LIGHT"] {
            assert_eq!(next_theme(start), "light", "start {start:?}");
            // …and toggling back must land on dark.
            assert_eq!(next_theme("light"), "dark", "toggling back from {start:?}");
        }
        assert_eq!(next_theme("light"), "dark");
    }

    /// An emptied field is "not set", not an empty string. The store's
    /// validation is shape-exact and rejects `""`, so committing one would
    /// fail the write and read to the user as a save error with no cause.
    #[test]
    fn an_emptied_field_is_unset_rather_than_an_empty_string() {
        assert_eq!(optional(String::new()), None);
        assert_eq!(optional("   ".into()), None);
        assert_eq!(
            optional("http://127.0.0.1:8080".into()).as_deref(),
            Some("http://127.0.0.1:8080")
        );
    }

    /// A field whose edit produced no change must still produce a payload
    /// equal to what is confirmed. This is what stops a blur-without-edit
    /// (click into a field, click out) from writing anything.
    #[test]
    fn editing_nothing_commits_nothing() {
        let out = commit_payload(&confirmed(), &confirmed(), SettingsField::RelayUrl);
        assert_eq!(out, confirmed());
    }
}
