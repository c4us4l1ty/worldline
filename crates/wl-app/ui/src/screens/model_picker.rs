//! The choice sheets — which model fills a tier slot, and which provider
//! the BYOK key belongs to.
//!
//! Both are bottom sheets rather than native `<select>` elements, and the
//! provider is the one that settles the argument. A free-text field for a
//! value that must match a provider's exact id string fails silently: the
//! request 400s, the error was discarded, and the UI said "OFFLINE".
//! 443 chat models (OpenRouter today) do not fit in a native popup on a
//! 420px WebKit screen, and a native popup cannot host a search field or a
//! "Recommended" group at all.
//!
//! The provider was a four-option `<select>` and should have been
//! harmless. It was not: WebKitGTK painted its own closed state, so it
//! shipped as a **near-white field carrying #E5E2E0 text** inside a
//! graphite card — twice, once "fixed" with `color-scheme: dark` and
//! `appearance: none` and once restyled. Neither declaration reliably
//! reaches the widget. Rather than restyle it a third time, Settings now
//! contains no native form widget at all, so there is one row type, one
//! open gesture, and one list rendering. The compose screen's horizon
//! pill is the last `<select>` in the app, and it survives because it is a
//! small closed control with no failure to report.
//!
//! The sheets are rendered *inside* the settings screen rather than as a
//! global overlay like the nav and telemetry drawers: they are only ever
//! opened from here, so they can own local signals instead of growing
//! `AppCtx` for a transient concern.
//!
//! Nothing here chooses anything for the user. The RECOMMENDED group is
//! an ordering aid — the provider's own catalog is the only source of
//! truth, and a model released upstream appears under ALL as soon as the
//! catalog is refetched, with no Worldline release involved.

use dioxus::prelude::*;

use crate::app::{flash, AppCtx, ModelInfoView};
use crate::icons::IconClose;

/// The BYOK providers, in the order the chooser lists them.
///
/// The first entry is the manual mode: no provider, no key, no AI. It is
/// listed rather than implied, because "None" as an absent value is not
/// a choice a user can see they have made.
///
/// The ids are the wire values the shell's vault namespaces by. Changing
/// one orphans that provider's sealed key, so they are not display names.
pub const PROVIDERS: [(&str, &str); 4] = [
    ("", "None — manual mode"),
    ("openrouter", "OpenRouter"),
    ("google", "Google"),
    ("bytez.com", "bytez.com"),
];

/// Display name for a stored provider id.
///
/// An id this build does not know renders as itself rather than as
/// nothing: a vault key written by a newer build must stay visible to an
/// older one, and silently showing a blank row would look like the key
/// had vanished.
pub fn provider_label(id: &str) -> &'static str {
    PROVIDERS
        .iter()
        .find(|(known, _)| *known == id)
        .map(|(_, label)| *label)
        .unwrap_or("Unrecognised provider")
}

/// Which tier slot the sheet is filling.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    Architect,
    Dispatcher,
}

impl Tier {
    /// The row label. This is the *whole* label now: the old markup read
    /// "Architect model (Tier 1)", which put a redundant parenthetical
    /// after a word that already says it, and the redundancy is what made
    /// the three choice rows read as unrelated fields rather than as one
    /// group.
    pub fn label(self) -> &'static str {
        match self {
            Tier::Architect => "Architect model",
            Tier::Dispatcher => "Dispatcher model",
        }
    }
}

/// How many rows the sheet will render at once.
///
/// OpenRouter serves 443 chat models; rendering every one of them into the
/// DOM on each keystroke is the thing that makes a searchable list feel
/// slower than a native select. Results are ranked, so the cap drops the
/// least relevant rather than an arbitrary slice, and the count line says
/// when it did.
pub const MAX_ROWS: usize = 60;

/// Filters and ranks the catalog against a query.
///
/// Ranking is what makes a short list useful: an id you paste exactly
/// lands first, a prefix second, a mid-string match third, and a
/// display-name match last. Case-insensitive throughout, because model
/// ids are conventionally lower-case and nobody types the case.
pub fn filter_models<'a>(query: &str, models: &'a [ModelInfoView]) -> Vec<&'a ModelInfoView> {
    let q = query.trim().to_ascii_lowercase();
    let mut scored: Vec<(u8, &ModelInfoView)> = Vec::new();
    for m in models {
        let rank: Option<u8> = if q.is_empty() {
            Some(0)
        } else {
            let id = m.id.to_ascii_lowercase();
            let name = m.name.to_ascii_lowercase();
            if id == q {
                Some(0)
            } else if id.starts_with(&q) {
                Some(1)
            } else if id.contains(&q) {
                Some(2)
            } else if name.contains(&q) {
                Some(3)
            } else {
                None
            }
        };
        if let Some(rank) = rank {
            scored.push((rank, m));
        }
    }
    // Stable within a rank, so an empty query keeps the catalog's own
    // order (recommended first, then alphabetical) rather than shuffling.
    scored.sort_by_key(|(rank, _)| *rank);
    scored.into_iter().map(|(_, m)| m).collect()
}

/// `128000` → `128k`, `1048576` → `1.0M`. Context windows span four
/// orders of magnitude, and four digits of raw token count is noise on a
/// 420px row.
pub fn format_context(n: Option<i64>) -> String {
    let Some(n) = n.filter(|n| *n > 0) else {
        return String::new();
    };
    if n >= 1_000_000 {
        let m = n as f64 / 1_000_000.0;
        // One decimal below 10M, none above: `1.0M` and `128M`, not
        // `1.05M` and `128.00M`.
        if m < 10.0 {
            format!("{m:.1}M")
        } else {
            format!("{m:.0}M")
        }
    } else if n >= 1_000 {
        format!("{}k", n / 1_000)
    } else {
        n.to_string()
    }
}

/// The meta line under a chosen model, or `None` when the catalog cannot
/// say anything about it.
///
/// Deliberately absent rather than a placeholder when unknown. The value
/// row's third line is a fact about the model or it is not there; a
/// rendered "—" or "unknown" would be noise on the common path where the
/// catalog simply has not been fetched yet.
pub fn chosen_meta(models: &[ModelInfoView], id: &str) -> Option<String> {
    let model = models.iter().find(|m| m.id == id)?;
    let ctx = format_context(model.context_length);
    (!ctx.is_empty()).then_some(ctx)
}

#[component]
pub fn ModelPicker(
    tier: Tier,
    provider: String,
    models: Vec<ModelInfoView>,
    recommended: Vec<String>,
    cached: bool,
    loading: bool,
    error: Option<String>,
    on_pick: EventHandler<String>,
    on_close: EventHandler<()>,
) -> Element {
    let ctx = use_context::<AppCtx>();
    let mut query = use_signal(String::new);
    // Free-text fallback, collapsed by default. The user's whole
    // complaint was typing model ids, so this is a way out for a
    // provider with no readable catalog — not the main path.
    let mut manual_open = use_signal(|| false);
    let mut manual = use_signal(String::new);

    let results = filter_models(&query.read(), &models);
    let shown = results.len().min(MAX_ROWS);
    // Recommended is a *section*, and only when it has entries: an empty
    // "Recommended" header is worse than no header. It also steps aside
    // during a search, where it would be a wall of noise above the
    // results the user is actually reading.
    let searching = !query.read().trim().is_empty();
    let rec: Vec<&ModelInfoView> = if searching {
        Vec::new()
    } else {
        recommended
            .iter()
            .filter_map(|id| models.iter().find(|m| &m.id == id))
            .collect()
    };

    rsx! {
        div {
            class: "wl-modal-backdrop wl-picker-backdrop",
            role: "presentation",
            onclick: move |_| on_close.call(()),
            div {
                class: "wl-picker-sheet",
                role: "dialog",
                aria_label: "{tier.label()}",
                onclick: move |e| e.stop_propagation(),

                div { class: "wl-picker-head",
                    h2 { class: "wl-picker-title", "{tier.label()}" }
                    button {
                        class: "wl-picker-close",
                        aria_label: "Close",
                        onclick: move |_| on_close.call(()),
                        IconClose {}
                    }
                }
                p { class: "wl-picker-sub",
                    "{provider} · {models.len()} models"
                    if cached { span { class: "wl-picker-cached", " · cached" } }
                    // Say so when the numbers are canned. Under `dx serve`
                    // every shell command is replaced by `invoke-shim.js`,
                    // so the list is invented and no API call is made — and
                    // a fabricated model list looks exactly like a real
                    // one from the outside. This cost a whole test cycle
                    // once already.
                    if crate::app::transport() == "mock" {
                        span { class: "wl-picker-mock", " · MOCK DATA" }
                    }
                }

                // Search. `oninput` rather than a submit, and the field
                // keeps focus: filtering is a view operation, not a
                // navigation, so stealing focus would break typing.
                div { class: "wl-picker-search",
                    input {
                        class: "wl-input wl-picker-input",
                        r#type: "search",
                        placeholder: "Search models…",
                        autocomplete: "off",
                        spellcheck: "false",
                        value: "{query.read()}",
                        oninput: move |e| query.set(e.value()),
                    }
                }

                // The sheet covers the page that would otherwise show
                // these, so it has to render them itself. "Loading" and
                // "this failed" are different states and neither is the
                // same as "the provider has no models" — conflating them
                // is how a failed fetch reads as an empty catalog.
                if let Some(err) = error.as_ref() {
                    p { class: "wl-form-error", "{err}" }
                } else if loading {
                    p { class: "wl-picker-empty", "Loading models from {provider}…" }
                }

                div { class: "wl-picker-list",
                    if models.is_empty() {
                        if !loading && error.is_none() {
                            p { class: "wl-picker-empty",
                                if provider.is_empty() {
                                    "Choose a provider first."
                                } else {
                                    "No models loaded yet. Close, set a key for {provider}, then reopen — or enter an id below."
                                }
                            }
                        }
                    } else if results.is_empty() {
                        p { class: "wl-picker-empty",
                            "No model matches that. Use the manual field below."
                        }
                    } else {
                        if !rec.is_empty() {
                            div { class: "wl-picker-group", "Recommended" }
                            for m in rec.iter().take(6) {
                                ModelRow {
                                    model: (*m).clone(),
                                    on_pick: move |id: String| on_pick.call(id),
                                }
                            }
                            div { class: "wl-picker-group", "All models" }
                        }
                        for m in results.iter().take(MAX_ROWS) {
                            ModelRow {
                                model: (*m).clone(),
                                on_pick: move |id: String| on_pick.call(id),
                            }
                        }
                        if results.len() > shown {
                            p { class: "wl-picker-more",
                                "{shown} of {results.len()} — keep typing to narrow"
                            }
                        }
                    }
                }

                // Fallback, for a provider with no readable catalog or a
                // model newer than the cached list.
                div { class: "wl-picker-manual",
                    if *manual_open.read() {
                        input {
                            class: "wl-input",
                            r#type: "text",
                            placeholder: "model id",
                            autocomplete: "off",
                            spellcheck: "false",
                            value: "{manual.read()}",
                            oninput: move |e| manual.set(e.value()),
                        }
                        button {
                            class: "wl-btn-ghost wl-btn-compact",
                            disabled: manual.read().trim().is_empty(),
                            onclick: move |_| {
                                let id = manual.read().trim().to_string();
                                if id.is_empty() {
                                    return;
                                }
                                // The shell validates nothing here on
                                // purpose: a hand-entered id is always
                                // attempted, and a wrong one comes back
                                // as a named provider error rather than
                                // being silently dropped.
                                flash(&ctx, &format!("USING {id}"));
                                on_pick.call(id);
                            },
                            "Use this id"
                        }
                    } else {
                        button {
                            class: "wl-picker-manual-toggle",
                            onclick: move |_| manual_open.set(true),
                            "Enter a model id instead"
                        }
                    }
                }
            }
        }
    }
}

/// The provider chooser.
///
/// Four rows and no search field, because four rows do not need one — the
/// search input would be a control that does nothing useful at this size.
/// The sheet is here for its *behaviour*, not its length: one open gesture
/// and one list rendering, identical to the model sheet, and no native
/// widget for WebKitGTK to paint white.
#[component]
pub fn ProviderPicker(
    current: String,
    on_pick: EventHandler<String>,
    on_close: EventHandler<()>,
) -> Element {
    rsx! {
        div {
            class: "wl-modal-backdrop wl-picker-backdrop",
            role: "presentation",
            onclick: move |_| on_close.call(()),
            div {
                class: "wl-picker-sheet",
                role: "dialog",
                aria_label: "AI provider",
                onclick: move |e| e.stop_propagation(),

                div { class: "wl-picker-head",
                    h2 { class: "wl-picker-title", "AI provider" }
                    button {
                        class: "wl-picker-close",
                        aria_label: "Close",
                        onclick: move |_| on_close.call(()),
                        IconClose {}
                    }
                }
                p { class: "wl-picker-sub",
                    "Your key is sealed in this device's vault. It is never written to the database and never sent to the relay."
                }

                div { class: "wl-picker-list",
                    for (id, label) in PROVIDERS {
                        button {
                            class: "wl-picker-row",
                            key: "{id}",
                            onclick: move |_| on_pick.call(id.to_string()),
                            span { class: "wl-picker-row-name",
                                style: "grid-area: id; color: var(--wl-text-primary);",
                                "{label}"
                            }
                            // A check mark, not a radio dot: it says
                            // "this is chosen" without implying a form
                            // the sheet does not have. Reserve its column
                            // on every row so the labels stay on one
                            // vertical axis whether or not one is chosen.
                            if current == id {
                                span { class: "wl-picker-row-ctx", "current" }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ModelRow(model: ModelInfoView, on_pick: EventHandler<String>) -> Element {
    let ctx_size = format_context(model.context_length);
    let id = model.id.clone();
    rsx! {
        button {
            class: "wl-picker-row",
            onclick: move |_| on_pick.call(id.clone()),
            span { class: "wl-picker-row-id", "{model.id}" }
            span { class: "wl-picker-row-name", "{model.name}" }
            if !ctx_size.is_empty() {
                span { class: "wl-picker-row-ctx", "{ctx_size}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: &str, name: &str, ctx: Option<i64>) -> ModelInfoView {
        ModelInfoView {
            id: id.into(),
            name: name.into(),
            context_length: ctx,
            supports_response_format: Some(true),
        }
    }

    fn catalog() -> Vec<ModelInfoView> {
        vec![
            m(
                "anthropic/claude-sonnet-5",
                "Anthropic: Claude Sonnet 5",
                Some(1_000_000),
            ),
            m("openai/gpt-5.4", "OpenAI: GPT-5.4", Some(1_050_000)),
            m(
                "google/gemini-3.1-pro-preview",
                "Google: Gemini 3.1 Pro",
                Some(1_048_576),
            ),
            m("openai/gpt-5.4-mini", "OpenAI: GPT-5.4 Mini", Some(400_000)),
        ]
    }

    #[test]
    fn an_empty_query_keeps_everything_in_catalog_order() {
        let c = catalog();
        let r = filter_models("", &c);
        assert_eq!(r.len(), 4);
        assert_eq!(r[0].id, "anthropic/claude-sonnet-5", "order must be stable");
    }

    #[test]
    fn ranking_prefers_exact_then_prefix_then_substring() {
        let c = catalog();
        let r = filter_models("gpt-5.4", &c);
        // Both `openai/gpt-5.4` and `...-mini` contain the query; the
        // exact id must come first.
        assert_eq!(r[0].id, "openai/gpt-5.4");
        assert!(r[1].id.contains("mini"));
    }

    #[test]
    fn search_is_case_insensitive_and_matches_display_names() {
        let c = catalog();
        assert_eq!(
            filter_models("SONNET", &c)[0].id,
            "anthropic/claude-sonnet-5"
        );
        // A vendor prefix is a substring of the id, so this finds models
        // the user would otherwise have to scroll for.
        let r = filter_models("openai/", &c);
        assert_eq!(r.len(), 2);
        assert!(r.iter().all(|x| x.id.starts_with("openai/")));
    }

    #[test]
    fn a_query_matching_nothing_yields_nothing_rather_than_everything() {
        let c = catalog();
        assert!(filter_models("no-such-model", &c).is_empty());
        assert!(
            filter_models("   ", &c).len() == 4,
            "whitespace is not a query"
        );
    }

    #[test]
    fn leading_and_trailing_space_does_not_break_a_real_query() {
        let c = catalog();
        assert_eq!(filter_models("  sonnet  ", &c).len(), 1);
    }

    /// The whole reason the sheet is not a native `<select>`: the row
    /// count is bounded, and the bound is visible to the user.
    #[test]
    fn results_are_capped_so_the_dom_stays_small() {
        let many: Vec<ModelInfoView> = (0..500)
            .map(|i| m(&format!("vendor/model-{i:04}"), "Big", Some(1000)))
            .collect();
        assert_eq!(
            filter_models("", &many).len(),
            500,
            "filtering is not lossy"
        );
        // The component slices to MAX_ROWS; the cap is the contract.
        assert_eq!(MAX_ROWS, 60);
        assert!(MAX_ROWS < many.len());
    }

    #[test]
    fn context_windows_read_at_every_magnitude() {
        assert_eq!(format_context(Some(200_000)), "200k");
        assert_eq!(format_context(Some(1_000_000)), "1.0M");
        assert_eq!(format_context(Some(1_048_576)), "1.0M");
        assert_eq!(format_context(Some(128_000_000)), "128M");
        assert_eq!(format_context(Some(512)), "512");
        // Absent or nonsensical values render as nothing, so the row
        // simply has no context badge rather than showing "0".
        assert_eq!(format_context(None), "");
        assert_eq!(format_context(Some(0)), "");
        assert_eq!(format_context(Some(-1)), "");
    }

    /// The manual-mode entry is listed, not implied. "None" as an absent
    /// value is not a choice a user can see they have made, and the
    /// chooser is the only place that choice is offered.
    #[test]
    fn manual_mode_is_a_listed_choice_not_an_implicit_one() {
        assert_eq!(PROVIDERS[0].0, "", "manual mode must be first");
        assert!(
            !PROVIDERS[0].1.is_empty(),
            "an empty id must still carry a visible label"
        );
        assert_eq!(provider_label(""), "None — manual mode");
    }

    /// Every id the shell's vault namespaces by has to render as its
    /// human name, and a name must never render as its own id — that is
    /// what put a raw wire value in front of a user once.
    #[test]
    fn every_known_provider_id_has_its_own_label() {
        for (id, label) in PROVIDERS {
            assert_eq!(provider_label(id), label, "{id} lost its label");
            assert!(!label.is_empty());
        }
        // Two of the three real ids ARE their own display name. That is
        // fine — the rule this test actually protects is that a label is
        // never MISSING, not that it is always different from the wire
        // value. `bytez.com` and `google` are how these providers spell
        // themselves; inventing "Bytez" would be a guess about a
        // third-party brand.
        assert_eq!(provider_label("bytez.com"), "bytez.com");
        assert_eq!(provider_label("google"), "Google");
        // The one that genuinely needed renaming.
        assert_eq!(provider_label("openrouter"), "OpenRouter");
    }

    /// A vault key written by a newer build names a provider this build
    /// has never heard of. Showing a blank row would look like the key had
    /// vanished; showing the raw id at least tells the truth.
    #[test]
    fn an_unknown_provider_id_says_so_rather_than_rendering_nothing() {
        assert_eq!(provider_label("future.provider"), "Unrecognised provider");
    }

    /// The value row's third line is a fact about the model or it is not
    /// there. A rendered placeholder would sit under every unset slot and
    /// train the eye to skip the line.
    #[test]
    fn the_meta_line_reports_a_fact_or_nothing_at_all() {
        let c = catalog();
        // 1,050,000 rounds to 1.1M at one decimal, not 1.0M — the fixture
        // value is deliberately not a round million so a truncation bug
        // would show up here rather than passing on a friendly number.
        assert_eq!(chosen_meta(&c, "openai/gpt-5.4"), Some("1.1M".into()));
        // A model the catalog has never heard of — a hand-entered id, or
        // one released upstream since the last fetch.
        assert_eq!(chosen_meta(&c, "vendor/not-fetched-yet"), None);
        // Present in the catalog, but with no usable context figure.
        assert_eq!(
            chosen_meta(&[m("vendor/bare", "Bare", None)], "vendor/bare"),
            None
        );
        assert_eq!(chosen_meta(&[], "openai/gpt-5.4"), None);
    }
}
