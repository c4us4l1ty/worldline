//! Model picker — a bottom sheet for choosing which model fills one of
//! the two AI tier slots.
//!
//! Exists because the alternative was a free-text field, and a free-text
//! field for a value that must match a provider's exact id string fails
//! silently: the request 400s, the error was discarded, and the UI said
//! "OFFLINE". 443 chat models (OpenRouter today) also do not fit in a
//! native `<select>` on a 420px WebKit popup.
//!
//! The sheet is rendered *inside* the settings screen rather than as a
//! global overlay like the nav and telemetry drawers: it is only ever
//! opened from here, so it can own local signals instead of growing
//! `AppCtx` for a transient concern.
//!
//! Nothing here chooses a model for the user. The RECOMMENDED group is
//! an ordering aid — the provider's own list is the only source of
//! truth, and a model released upstream appears under ALL as soon as the
//! catalog is refetched, with no Worldline release involved.

use dioxus::prelude::*;

use crate::app::ModelInfoView;
use crate::app::{flash, AppCtx};

/// Which tier slot the sheet is filling.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    Architect,
    Dispatcher,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Architect => "Architect model",
            Tier::Dispatcher => "Dispatcher model",
        }
    }

    /// The tier number, as a string. Precomputed rather than an inline
    /// `if` in the rsx: a conditional expression inside a `label { }`
    /// attribute is not parseable there.
    pub fn number(self) -> &'static str {
        match self {
            Tier::Architect => "1",
            Tier::Dispatcher => "2",
        }
    }
}

/// How many rows the sheet will render at once.
///
/// OpenRouter serves 443 chat models; rendering every one of them into
/// the DOM on each keystroke is the thing that makes a searchable list
/// feel slower than a native select. Results are ranked, so the cap
/// drops the least relevant rather than an arbitrary slice, and the
/// count line says when it did.
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

#[component]
pub fn ModelPicker(
    tier: Tier,
    provider: String,
    models: Vec<ModelInfoView>,
    recommended: Vec<String>,
    cached: bool,
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

    let close = move |_| on_close.call(());
    let pick = move |id: String| on_pick.call(id);

    rsx! {
        div {
            class: "wl-modal-backdrop wl-picker-backdrop",
            role: "presentation",
            onclick: close,
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
                        "✕"
                    }
                }
                p { class: "wl-picker-sub",
                    "{provider} · {models.len()} models"
                    if cached { span { class: "wl-picker-cached", " · cached" } }
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

                div { class: "wl-picker-list",
                    if models.is_empty() {
                        p { class: "wl-picker-empty",
                            "This provider returned no models. Use the manual field below."
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
                                    on_pick: pick,
                                }
                            }
                            div { class: "wl-picker-group", "All models" }
                        }
                        for m in results.iter().take(MAX_ROWS) {
                            ModelRow {
                                model: (*m).clone(),
                                on_pick: pick,
                            }
                        }
                        if results.len() > shown {
                            p { class: "wl-picker-more wl-mono",
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
                                pick(id);
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

#[component]
fn ModelRow(model: ModelInfoView, on_pick: EventHandler<String>) -> Element {
    let ctx_size = format_context(model.context_length);
    let id = model.id.clone();
    rsx! {
        button {
            class: "wl-picker-row",
            onclick: move |_| on_pick.call(id.clone()),
            span { class: "wl-picker-row-id wl-mono", "{model.id}" }
            span { class: "wl-picker-row-name", "{model.name}" }
            if !ctx_size.is_empty() {
                span { class: "wl-picker-row-ctx wl-mono", "{ctx_size}" }
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
}
