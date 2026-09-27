//! Provider model catalog (PRD §4, BYOK).
//!
//! Two jobs, both of which exist because "type the model id by hand" is
//! not a usable configuration surface:
//!
//! 1. **Discovery.** [`models_endpoint`] + the `parse_*` family turn a
//!    provider's `/models` response into a flat, bounded
//!    [`ModelInfo`] list, so the UI offers real ids instead of a text
//!    field where a typo costs an API call to discover.
//! 2. **Diagnosis.** [`provider_error_message`] turns a provider's HTTP
//!    failure into something a person can act on. Before this, every
//!    failure — bad key, bad model, no network — reached the UI as the
//!    bare string `HTTP 400`, because the response body was discarded.
//!
//! **Everything here is live.** There is deliberately no bundled
//! fallback catalog: a static list is stale the moment a provider ships
//! a model, and the whole point of the catalog is that a newly released
//! model appears without a Worldline release. The only static data in
//! this file is [`curated`], which is a *display* aid (the "Recommended"
//! group at the top of the picker) and gates nothing — see its docs.
//!
//! Pure and platform-clean: parsing, filtering and message formatting
//! are all total functions over strings, so the whole surface is
//! unit-tested without a network, a key, or a provider.

use serde::{Deserialize, Serialize};

use crate::domain::MAX_MODEL_ID_CHARS;

/// OpenAI-compatible chat-completions base URL per approved provider id.
///
/// Lives in `wl-core` (not the shell) because the catalog needs it and
/// because a base-URL table is exactly the kind of thing that should be
/// covered by a test rather than a comment. `None` for an unknown id —
/// never a silent fallback, so a typo cannot be routed somewhere real.
pub fn base_for(provider: &str) -> Option<String> {
    match provider {
        "openrouter" => Some("https://openrouter.ai/api/v1".into()),
        "google" => Some("https://generativelanguage.googleapis.com/v1beta/openai".into()),
        "bytez.com" => Some("https://api.bytez.com/models/v2/openai/v1".into()),
        _ => None,
    }
}

/// Where a provider's model list lives, and whether it needs a key.
pub struct ModelsEndpoint {
    /// Absolute URL to GET.
    pub url: String,
    /// `true` when the provider rejects an unauthenticated list request.
    pub needs_key: bool,
    /// `true` when the response carries modality/parameter metadata
    /// (OpenRouter). When `false`, [`parse_openai_list`] applies a
    /// display-only non-chat denylist instead, because the provider
    /// cannot tell us what is chat-capable.
    pub rich_metadata: bool,
}

/// Model-list endpoint for a provider, or `None` if it is not approved.
pub fn models_endpoint(provider: &str) -> Option<ModelsEndpoint> {
    let base = base_for(provider)?;
    Some(ModelsEndpoint {
        url: format!("{base}/models"),
        // OpenRouter's catalog is public; the other two 401 without a
        // key. Gating all three on a stored key anyway is deliberate:
        // one rule for the user to learn instead of three, and it stops
        // a keyless install pulling a ~750 KB catalog it cannot use.
        needs_key: true,
        rich_metadata: provider == "openrouter",
    })
}

/// Upper bound on retained models. OpenRouter serves 458 today and
/// grows continuously; 4096 is a guard against a hostile or broken
/// endpoint, not a real limit. When the cap bites, the caller reports
/// `truncated` so the UI can say so rather than implying the list is
/// complete.
pub const MAX_MODELS: usize = 4096;

/// Display-name cap. Provider names are short labels; anything longer is
/// a description that has leaked into the wrong field.
const MAX_NAME_CHARS: usize = 160;

/// One selectable model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    /// The exact id to send as `model`. Never display-transformed.
    pub id: String,
    /// Human label, falling back to the id when the provider omits it.
    pub name: String,
    /// Context window in tokens, when the provider reports one.
    #[serde(default)]
    pub context_length: Option<i64>,
    /// Whether the provider advertises `response_format` for this
    /// model. `None` = the provider does not say, and the request keeps
    /// the historical behaviour of sending it (see
    /// [`json_mode_for`]) — we only ever *drop* the field on positive
    /// evidence that it will be rejected.
    #[serde(default)]
    pub supports_response_format: Option<bool>,
}

/// Whether to send `response_format: {"type":"json_object"}`.
///
/// Only *known-unsupported* turns it off. Unknown (`None`) keeps the
/// status quo, so Google and bytez.com — which do not publish the flag
/// — behave exactly as they did before this existed, and a model that
/// quietly stops accepting it is not a regression we introduced.
pub fn json_mode_for(info: Option<&ModelInfo>) -> bool {
    info.and_then(|i| i.supports_response_format) != Some(false)
}

/// A model id we are willing to store and send.
///
/// OpenRouter ids are `vendor/slug` with optional `:variant` suffixes and
/// a `~` prefix on some router ids; Google's are bare after the
/// `models/` prefix is stripped. That is the whole accepted alphabet.
/// The point of the charset is not to validate the provider — it is to
/// keep a hostile or broken endpoint from injecting a control character
/// or a newline into a value that ends up in SQLite, a log line and an
/// HTTP body.
pub fn valid_model_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_MODEL_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '~' | ':' | '/'))
}

/// Model families that are definitely not chat completions, used ONLY
/// to keep them out of the picker for providers whose list carries no
/// modality metadata (Google, bytez.com). It filters what is *shown*,
/// never what may be *used*: a manually entered id is always passed
/// through untouched, because guessing wrong here would hide a model
/// that works, and the user's ability to pick a new model the moment it
/// ships is the whole feature.
const NON_CHAT_HINTS: &[&str] = &[
    "embed",
    "tts",
    "whisper",
    "imagen",
    "lyria",
    "aqa",
    "onnx",
    "bison",
    "moderation",
    "dalle",
    "veo",
    "text-to-speech",
    "text-to-image",
];

fn looks_non_chat(id: &str) -> bool {
    let id = id.to_ascii_lowercase();
    NON_CHAT_HINTS.iter().any(|h| id.contains(h))
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CatalogError {
    /// The provider answered with an error object rather than a list.
    #[error("provider returned an error: {0}")]
    Provider(String),
    /// A 2xx response whose shape we do not recognise. Deliberately
    /// distinct from an empty list: "we could not read this" and "this
    /// provider has no models" are different bugs with different fixes.
    #[error("provider returned an unrecognised model list")]
    Shape,
    /// A well-formed list with nothing in it — usually a wrong or
    /// revoked key against a key-gated provider.
    #[error("provider returned no models")]
    Empty,
}

// ---------------------------------------------------------------------------
// OpenRouter: the rich shape
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct OrList {
    data: Option<Vec<OrModel>>,
    #[serde(default)]
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct OrModel {
    id: Option<String>,
    name: Option<String>,
    #[serde(default)]
    context_length: Option<i64>,
    #[serde(default)]
    architecture: Option<OrArch>,
    #[serde(default)]
    supported_parameters: Vec<String>,
}

#[derive(Deserialize)]
struct OrArch {
    #[serde(default)]
    output_modalities: Vec<String>,
}

/// Parses OpenRouter's `GET /v1/models`.
///
/// Keeps only text-output models. The live catalog contains image and
/// audio-output models (`google/gemini-3-pro-image`, `openai/gpt-audio`,
/// …) that return 400 on a chat-completions call, and offering them
/// would be offering a guaranteed failure.
///
/// Every field is optional on purpose. A new OpenRouter release that adds
/// a field, renames one, or ships an entry with an unexpected shape
/// degrades that entry — it never fails the whole list, because a
/// catalog that hard-fails on an upstream schema change is exactly how
/// new models stop appearing.
pub fn parse_openrouter(body: &str) -> Result<Vec<ModelInfo>, CatalogError> {
    let list: OrList = serde_json::from_str(body).map_err(|_| CatalogError::Shape)?;
    let Some(entries) = list.data else {
        if let Some(msg) = list.error.as_ref().and_then(error_message_in) {
            return Err(CatalogError::Provider(msg));
        }
        return Err(CatalogError::Shape);
    };
    let mut out: Vec<ModelInfo> = Vec::with_capacity(entries.len().min(MAX_MODELS));
    // Stops as soon as the cap is reached rather than walking a whole
    // hostile list: `entries` is already materialised by `from_str`, but
    // building a 400 000-element output Vec before truncating it was
    // pure amplification of a hostile body.
    for e in entries {
        if out.len() >= MAX_MODELS {
            break;
        }
        let Some(id) = e.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
            continue;
        };
        if !valid_model_id(id) {
            continue;
        }
        // Text-output only. An entry with no `output_modalities` at all
        // is kept: absence is not evidence of non-chat, and dropping it
        // would hide a model that works.
        if let Some(arch) = &e.architecture {
            if !arch.output_modalities.is_empty() && arch.output_modalities != ["text"] {
                continue;
            }
        }
        let name = e
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| truncate(s, MAX_NAME_CHARS))
            .unwrap_or_else(|| id.to_string());
        out.push(ModelInfo {
            id: id.to_string(),
            name,
            context_length: e.context_length,
            supports_response_format: Some(
                e.supported_parameters
                    .iter()
                    .any(|p| p == "response_format"),
            ),
        });
    }
    Ok(dedupe(out))
}

// ---------------------------------------------------------------------------
// Google + bytez.com: the plain OpenAI shape
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct OaList {
    data: Option<Vec<OaModel>>,
    /// Google's *native* `models.list` uses `models[]` with `name`.
    /// Supporting it costs three lines and means the catalog keeps
    /// working if the OpenAI-compat route is ever retired.
    models: Option<Vec<OaModel>>,
}

#[derive(Deserialize)]
struct OaModel {
    id: Option<String>,
    name: Option<String>,
    #[serde(default)]
    supported_parameters: Vec<String>,
}

/// Parses a plain OpenAI-compatible `{"data":[{"id":…}]}` list, plus
/// Google's `{"models":[{"name":"models/…"}]}` variant.
///
/// Google's OpenAI-compat route returns `"id": "models/gemini-2.5-pro"`
/// — the chat-completions call needs the bare `gemini-2.5-pro`, so the
/// prefix is stripped here rather than being a rule the caller has to
/// remember. Anything without a chat-capable hint is kept; these
/// providers report no modality metadata, so the non-chat families are
/// filtered by name (display only — see [`NON_CHAT_HINTS`]).
pub fn parse_openai_list(body: &str) -> Result<Vec<ModelInfo>, CatalogError> {
    let list: OaList = serde_json::from_str(body).map_err(|_| CatalogError::Shape)?;
    let entries = list.data.or(list.models).ok_or(CatalogError::Shape)?;
    let mut out: Vec<ModelInfo> = Vec::with_capacity(entries.len().min(MAX_MODELS));
    for e in entries {
        if out.len() >= MAX_MODELS {
            break;
        }
        let raw =
            e.id.as_deref()
                .or(e.name.as_deref())
                .map(str::trim)
                .unwrap_or("");
        let id = raw.strip_prefix("models/").unwrap_or(raw);
        if !valid_model_id(id) {
            continue;
        }
        if looks_non_chat(id) {
            continue;
        }
        let name = e
            .name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty() && !s.starts_with("models/"))
            .map(|s| truncate(s, MAX_NAME_CHARS))
            .unwrap_or_else(|| id.to_string());
        out.push(ModelInfo {
            id: id.to_string(),
            name,
            context_length: None,
            supports_response_format: if e.supported_parameters.is_empty() {
                None
            } else {
                Some(
                    e.supported_parameters
                        .iter()
                        .any(|p| p == "response_format"),
                )
            },
        });
    }
    Ok(dedupe(out))
}

/// Parses a provider's list with the right reader for that provider.
pub fn parse_for(provider: &str, body: &str) -> Result<Vec<ModelInfo>, CatalogError> {
    if provider == "openrouter" {
        parse_openrouter(body)
    } else {
        parse_openai_list(body)
    }
}

/// Drops duplicate ids, keeping the first, and caps the result.
///
/// The cap is applied *while* de-duplicating, not after it. The old
/// shape was `Vec<String>` + linear scan per element, with the truncate
/// last: so a 4 MiB `/models` body (inside the shell's read ceiling,
/// which is the only bound on this path) of ~400 000 well-formed
/// entries cost ~8 × 10¹⁰ string comparisons — minutes of pinned CPU on
/// a UI thread — after both the entry list and the output had been
/// fully materialised. The legitimate 458-model OpenRouter catalog
/// never showed it, which is why a two-entry test could not.
///
/// A `HashSet` keyed on the id gives O(n) overall, and taking only the
/// first `MAX_MODELS` distinct ids means the loop stops early too.
fn dedupe(v: Vec<ModelInfo>) -> Vec<ModelInfo> {
    let mut seen: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(v.len().min(MAX_MODELS));
    v.into_iter()
        .filter(|m| seen.insert(m.id.clone()))
        .take(MAX_MODELS)
        .collect()
}

// ---------------------------------------------------------------------------
// The "Recommended" group — display only
// ---------------------------------------------------------------------------

/// Ids shown in the picker's RECOMMENDED group, best-first.
///
/// **This gates nothing.** It is a display ordering: an id here that the
/// provider has dropped simply does not match and is skipped, an id it
/// has never heard of changes nothing, and every model that survives
/// parsing is always listed under ALL. It is deliberately *not* an
/// allow-list, and nothing in the request path consults it — a
/// hand-entered id, a brand-new release, and a model that gains
/// `response_format` support next month all behave identically.
///
/// `:batch` variants are excluded by hand: they are offline batch
/// endpoints and are the wrong default for an interactive planner.
pub fn curated(provider: &str) -> &'static [&'static str] {
    match provider {
        // Verified present in the live OpenRouter catalog, and the first
        // entries that do support `response_format`.
        "openrouter" => &[
            "anthropic/claude-sonnet-5",
            "openai/gpt-5.4",
            "google/gemini-3.1-pro-preview",
            "openai/gpt-5.4-mini",
            "google/gemini-3.1-flash-lite",
            "anthropic/claude-sonnet-4.6",
        ],
        // Google's list is key-gated, so these cannot be verified without
        // a key. If none match, the group is omitted rather than shown
        // empty.
        "google" => &["gemini-3.1-pro-preview", "gemini-3-flash-preview"],
        // bytez.com publishes no catalog we can read anonymously; the
        // picker shows ALL only, which is the honest state.
        _ => &[],
    }
}

/// Resolves [`curated`] against a live catalog, in curated order,
/// skipping ids the provider no longer lists. Empty means "omit the
/// group entirely".
pub fn recommended<'a>(provider: &str, catalog: &'a [ModelInfo]) -> Vec<&'a ModelInfo> {
    curated(provider)
        .iter()
        .filter_map(|want| catalog.iter().find(|m| m.id == *want))
        .collect()
}

// ---------------------------------------------------------------------------
// Provider errors → something a person can act on
// ---------------------------------------------------------------------------

/// Longest error string we will hand back. Long enough for a provider's
/// own sentence, short enough for a single-line toast.
const MAX_ERROR_CHARS: usize = 240;

/// Strips anything shaped like a credential out of a string that is
/// about to be shown to a person.
///
/// Two classes, because providers quote the key back in two shapes:
/// the real one, which is `sk-`/`sk-or-v1-`/`AIza`-prefixed with a long
/// opaque body, and a truncation of it. Long enough that an ordinary
/// word or a model id cannot match, short enough to catch the
/// `sk-or-v1-1a2b3c…` prefixes and suffixes gateways quote.
fn redact_credentials(s: &str) -> String {
    const PREFIXES: [&str; 5] = ["sk-", "sk-or-", "AIza", "gsk_", "xai-"];
    const MIN_BODY: usize = 8;
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let rest = &s[i..];
        let hit = PREFIXES
            .iter()
            .filter_map(|p| rest.strip_prefix(*p))
            .find(|after| {
                after
                    .bytes()
                    .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
                    .count()
                    >= MIN_BODY
            });
        match hit {
            Some(after) => {
                let run = after
                    .bytes()
                    .take_while(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
                    .count();
                let prefix_len = rest.len() - after.len();
                out.push_str(&rest[..prefix_len]);
                out.push_str("[redacted]");
                i += prefix_len + run;
            }
            None => {
                let ch = rest.chars().next().expect("non-empty by construction");
                out.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    out
}

/// Turns a failed AI/provider call into an actionable sentence.
///
/// This exists because the previous behaviour returned
/// `format!("HTTP {}", status)` and dropped the body, so a wrong model
/// id and a wrong key were indistinguishable — and both looked like
/// "OFFLINE" in the UI. The provider usually *says* exactly what is
/// wrong ("No endpoints found matching model 'flagship'"), so the body
/// is the diagnosis and the status is only the framing.
pub fn provider_error_message(provider: &str, status: u16, body: &str) -> String {
    let detail = error_message_of(body)
        .or_else(|| non_json_detail(body))
        .unwrap_or_default();
    // Redacted before it is put anywhere, not after. A wrong-key 401
    // from an OpenAI-compatible gateway is literally
    // `{"error":{"message":"Incorrect API key provided: sk-or-v1-1a2b3c…  "}}`
    // — the key's own prefix and suffix — and this string crosses the
    // IPC boundary into the webview, where it is rendered in a toast.
    // `AGENTS.md` forbids secrets reaching the DOM, and the BYOK key is
    // the one secret a provider is guaranteed to quote back at us. So
    // the credential shape is stripped here, at the place that first
    // holds a provider's raw body, and the diagnosis itself survives
    // (which half of the key is wrong is not useful to a user anyway —
    // regenerate it).
    let detail = redact_credentials(&detail);
    let lead = match status {
        400 => "bad request",
        401 => "key rejected",
        403 => "key not permitted",
        404 => "endpoint or model not found",
        413 => "request too large",
        422 => "request rejected",
        429 => "rate limited or out of quota",
        500..=599 => "provider error",
        _ => "request failed",
    };
    let mut msg = if detail.is_empty() {
        format!("{lead} (HTTP {status})")
    } else {
        format!("{lead}: {detail}")
    };
    // Google stopped accepting unrestricted Gemini API keys on
    // 2026-06-19, which produces a bare 403 that means nothing to
    // anyone who has not read that changelog.
    if status == 403 && provider == "google" {
        msg.push_str(" — Google now requires an API-restricted key");
    }
    truncate(&collapse_ws(&msg), MAX_ERROR_CHARS)
}

/// Pulls a human message out of the error shapes these providers
/// actually use: OpenAI/OpenRouter (`error.message` or a bare string),
/// Google (`error.message` with `code`/`status`), bytez
/// (`error.message` alongside `error.unknown`), plus a bare
/// `message`/`detail` at the top level.
fn error_message_of(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    error_message_in(&v)
}

fn error_message_in(v: &serde_json::Value) -> Option<String> {
    let m = v.as_object()?;
    let candidate = m
        .get("error")
        .and_then(|e| match e {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Object(o) => o
                .get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string),
            _ => None,
        })
        .or_else(|| {
            m.get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .or_else(|| m.get("detail").and_then(|m| m.as_str()).map(str::to_string))?;
    let s = collapse_ws(candidate.trim());
    (!s.is_empty()).then_some(s)
}

/// Fallback for a non-JSON body. A proxy or gateway in front of a
/// provider answers with an HTML error page, and dumping `<html>…` at a
/// user helps nobody — so tags are stripped and the remaining text is
/// used ("Bad Gateway" out of `<body>Bad Gateway</body>`). An HTML
/// `<title>` is the most informative part of such a page, so it is
/// preferred when present.
fn non_json_detail(body: &str) -> Option<String> {
    if let Some(title) = html_tag_text(body, "title") {
        let t = collapse_ws(&title);
        if !t.is_empty() {
            return Some(truncate(&t, MAX_ERROR_CHARS));
        }
    }
    let stripped = strip_tags(&body.chars().take(4096).collect::<String>());
    let s = collapse_ws(&stripped);
    (!s.is_empty()).then(|| truncate(&s, MAX_ERROR_CHARS))
}

/// Text content of the first `<tag>…</tag>`, if the body contains one.
fn html_tag_text(body: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let start = body.to_ascii_lowercase().find(&open)? + open.len();
    let end = body[start..].to_ascii_lowercase().find(&close)? + start;
    Some(strip_tags(&body[start..end]))
}

/// Removes `<…>` spans. Deliberately crude — this runs on an error path
/// for a body that is already known not to be JSON, and its only job is
/// to keep markup out of a one-line toast.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::KNOWN_PROVIDERS;

    /// Trimmed from the live OpenRouter catalog, keeping every case the
    /// parser has to distinguish: text-out, image-out, audio-out, a
    /// model with no modality block, a model with no supported
    /// parameters, a `~` router id, and a `:batch` variant.
    const OR_FIXTURE: &str = r#"{"data":[
      {"id":"anthropic/claude-sonnet-4","name":"Anthropic: Claude Sonnet 4",
       "context_length":200000,"architecture":{"output_modalities":["text"]},
       "supported_parameters":["max_tokens","tools"]},
      {"id":"anthropic/claude-sonnet-5","name":"Anthropic: Claude Sonnet 5",
       "context_length":1000000,"architecture":{"output_modalities":["text"]},
       "supported_parameters":["max_tokens","response_format"]},
      {"id":"anthropic/claude-sonnet-5:batch","name":"Anthropic: Claude Sonnet 5 (batch)",
       "context_length":1000000,"architecture":{"output_modalities":["text"]},
       "supported_parameters":["max_tokens","response_format"]},
      {"id":"openrouter/auto-beta","name":"Auto",
       "architecture":{"output_modalities":["text"]},"supported_parameters":[]},
      {"id":"~deepseek/deepseek-pro-latest","name":"DeepSeek Pro",
       "architecture":{"output_modalities":["text"]},"supported_parameters":["response_format"]},
      {"id":"google/gemini-3-pro-image","name":"Gemini 3 Pro Image",
       "architecture":{"output_modalities":["image","text"]},"supported_parameters":[]},
      {"id":"openai/gpt-audio","name":"GPT Audio",
       "architecture":{"output_modalities":["text","audio"]},"supported_parameters":[]},
      {"id":"future/unreleased-shape","architecture":{"output_modalities":["text"]}},
      {"id":"bad id with spaces","name":"Nope"},
      {"id":"","name":"Empty"},
      {"name":"No id at all"}
    ]}"#;

    #[test]
    fn only_text_output_models_survive() {
        let m = parse_openrouter(OR_FIXTURE).unwrap();
        let ids: Vec<&str> = m.iter().map(|x| x.id.as_str()).collect();
        assert!(ids.contains(&"anthropic/claude-sonnet-4"));
        assert!(
            !ids.contains(&"google/gemini-3-pro-image"),
            "image-out must be dropped"
        );
        assert!(
            !ids.contains(&"openai/gpt-audio"),
            "audio-out must be dropped"
        );
        // 5 survivors: sonnet-4, sonnet-5, :batch, auto-beta, ~deepseek,
        // future/unreleased-shape == 6.
        assert_eq!(ids.len(), 6, "{ids:?}");
    }

    #[test]
    fn malformed_entries_are_skipped_not_fatal() {
        let m = parse_openrouter(OR_FIXTURE).unwrap();
        let ids: Vec<&str> = m.iter().map(|x| x.id.as_str()).collect();
        assert!(
            !ids.iter().any(|i| i.contains(' ')),
            "space-bearing id dropped"
        );
        assert!(!ids.contains(&""), "empty id dropped");
    }

    #[test]
    fn tilde_router_ids_are_kept_verbatim() {
        let m = parse_openrouter(OR_FIXTURE).unwrap();
        assert!(m.iter().any(|x| x.id == "~deepseek/deepseek-pro-latest"));
    }

    #[test]
    fn a_new_upstream_shape_never_breaks_the_list() {
        // An unknown extra field, and a model with no `architecture`
        // block at all, both parse.
        let m = parse_openrouter(
            r#"{"data":[{"id":"x/y","brand_new_field":{"a":1}},
                        {"id":"x/z","architecture":{"brand_new":"shape"}}]}"#,
        )
        .unwrap();
        assert_eq!(m.len(), 2);
        // Absent architecture is not evidence of non-chat: keep it.
        assert!(m.iter().any(|x| x.id == "x/y"));
        // Absent name falls back to the id, so the row is never blank.
        assert!(m.iter().all(|x| !x.name.is_empty()));
    }

    #[test]
    fn response_format_support_is_read_from_supported_parameters() {
        let m = parse_openrouter(OR_FIXTURE).unwrap();
        let get = |id: &str| m.iter().find(|x| x.id == id).unwrap().clone();
        assert_eq!(
            get("anthropic/claude-sonnet-4").supports_response_format,
            Some(false)
        );
        assert_eq!(
            get("anthropic/claude-sonnet-5").supports_response_format,
            Some(true)
        );
        assert_eq!(
            get("openrouter/auto-beta").supports_response_format,
            Some(false)
        );
    }

    /// The flagship that motivated this: a real, popular model that
    /// rejects `response_format`, so the unconditional field was a
    /// guaranteed 400 for anyone who picked it.
    #[test]
    fn json_mode_is_dropped_only_on_positive_evidence() {
        let off = ModelInfo {
            id: "anthropic/claude-sonnet-4".into(),
            name: "x".into(),
            context_length: None,
            supports_response_format: Some(false),
        };
        let unknown = ModelInfo {
            supports_response_format: None,
            ..off.clone()
        };
        let on = ModelInfo {
            supports_response_format: Some(true),
            ..off.clone()
        };
        assert!(!json_mode_for(Some(&off)));
        assert!(json_mode_for(Some(&unknown)), "unknown keeps status quo");
        assert!(json_mode_for(Some(&on)));
        assert!(json_mode_for(None), "no catalog entry keeps status quo");
    }

    #[test]
    fn openrouter_shape_errors_are_distinguishable_from_empty() {
        assert_eq!(parse_openrouter("{}"), Err(CatalogError::Shape));
        assert_eq!(parse_openrouter("not json"), Err(CatalogError::Shape));
        assert_eq!(parse_openrouter(r#"{"data":[]}"#), Ok(vec![]));
        assert_eq!(
            parse_openrouter(r#"{"error":{"message":"nope"}}"#),
            Err(CatalogError::Provider("nope".into()))
        );
    }

    #[test]
    fn google_ids_lose_the_models_prefix() {
        let m = parse_openai_list(
            r#"{"object":"list","data":[
              {"id":"models/gemini-2.5-pro","object":"model","owned_by":"google"},
              {"id":"models/gemini-embedding-001","object":"model"},
              {"id":"gemini-2.5-flash","object":"model"}]}"#,
        )
        .unwrap();
        let ids: Vec<&str> = m.iter().map(|x| x.id.as_str()).collect();
        assert!(ids.contains(&"gemini-2.5-pro"));
        assert!(
            ids.contains(&"gemini-2.5-flash"),
            "unprefixed id passes through"
        );
        assert!(!ids.iter().any(|i| i.starts_with("models/")));
        assert!(
            !ids.contains(&"gemini-embedding-001"),
            "non-chat family hidden"
        );
    }

    #[test]
    fn google_native_models_array_is_accepted() {
        let m = parse_openai_list(
            r#"{"models":[{"name":"models/gemini-3-pro","displayName":"Gemini 3 Pro"}]}"#,
        )
        .unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].id, "gemini-3-pro");
    }

    #[test]
    fn duplicates_collapse_keeping_the_first() {
        let m = parse_openrouter(
            r#"{"data":[{"id":"a/b","name":"first"},
                        {"id":"a/b","name":"second"}]}"#,
        )
        .unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].name, "first");
    }

    #[test]
    fn curated_ids_never_gate_anything() {
        let m = parse_openrouter(OR_FIXTURE).unwrap();
        let rec = recommended("openrouter", &m);
        // Only ids actually present come back, in curated order.
        assert!(rec.iter().all(|r| m.iter().any(|x| x.id == r.id)));
        assert!(rec.iter().any(|r| r.id == "anthropic/claude-sonnet-5"));
        // A brand-new model the curated list has never heard of is still
        // selectable — that is the whole point of the catalog.
        assert!(m.iter().any(|x| x.id == "future/unreleased-shape"));
        assert!(!rec.iter().any(|r| r.id == "future/unreleased-shape"));
        // bytez has no curated list: omitted, not empty-but-shown.
        assert!(recommended("bytez.com", &m).is_empty());
    }

    #[test]
    fn curated_excludes_batch_variants() {
        for id in curated("openrouter") {
            assert!(!id.ends_with(":batch"), "{id} is a batch endpoint");
            assert!(!id.contains(' '));
        }
    }

    #[test]
    fn model_id_charset_and_length() {
        assert!(valid_model_id("anthropic/claude-sonnet-4.5:batch"));
        assert!(valid_model_id("~deepseek/deepseek-pro-latest"));
        assert!(valid_model_id("gemini-2.5-pro"));
        assert!(!valid_model_id(""));
        assert!(!valid_model_id("has space"));
        assert!(!valid_model_id("new\nline"));
        assert!(!valid_model_id("semi;colon"));
        assert!(!valid_model_id(&"x".repeat(MAX_MODEL_ID_CHARS + 1)));
        assert!(valid_model_id(&"x".repeat(MAX_MODEL_ID_CHARS)));
    }

    #[test]
    fn every_known_provider_has_a_base_and_a_models_endpoint() {
        for p in KNOWN_PROVIDERS {
            assert!(base_for(p).is_some(), "{p} has no base url");
            assert!(base_for(p).unwrap().starts_with("https://"));
            let ep = models_endpoint(p).expect("models endpoint");
            assert!(ep.url.ends_with("/models"));
            assert!(ep.needs_key);
        }
        assert!(base_for("qwen").is_none(), "qwen was removed");
        assert!(base_for("openai").is_none(), "no silent fallback");
        assert!(models_endpoint("qwen").is_none());
    }

    #[test]
    fn error_messages_name_the_actual_problem() {
        // The case that started this: a bad model id.
        let m = provider_error_message(
            "openrouter",
            404,
            r#"{"error":{"message":"No endpoints found matching model 'flagship'"}}"#,
        );
        assert!(m.contains("not found"), "{m}");
        assert!(m.contains("flagship"), "{m}");

        // A bad key.
        let m = provider_error_message(
            "openrouter",
            401,
            r#"{"error":"No auth credentials found"}"#,
        );
        assert!(m.starts_with("key rejected"), "{m}");
        assert!(m.contains("No auth credentials found"), "{m}");

        // Google's key-restriction 403 says what to do about it.
        let m =
            provider_error_message("google", 403, r#"{"error":{"message":"PermissionDenied"}}"#);
        assert!(m.contains("API-restricted"), "{m}");
        // ...and only for Google.
        let m = provider_error_message("openrouter", 403, "{}");
        assert!(!m.contains("API-restricted"), "{m}");

        // bytez's shape.
        let m = provider_error_message(
            "bytez.com",
            401,
            r#"{"error":{"unknown":true,"message":"StatusCode: non 2xx"}}"#,
        );
        assert!(m.contains("StatusCode"), "{m}");

        // No body at all still says something useful.
        let m = provider_error_message("openrouter", 429, "");
        assert!(m.contains("rate limited"), "{m}");
        assert!(m.contains("429"), "{m}");

        // An HTML proxy page must yield its text, not its markup.
        let m = provider_error_message(
            "openrouter",
            502,
            "<html><head><title>502 Bad Gateway</title></head><body><h1>Bad Gateway</h1></body></html>",
        );
        assert!(m.contains("502 Bad Gateway"), "{m}");
        assert!(!m.contains('<'), "{m}");
        assert!(!m.contains(">"), "{m}");
    }

    #[test]
    fn error_messages_are_bounded_and_single_line() {
        let long = "x".repeat(2000);
        let m = provider_error_message("openrouter", 400, &format!(r#"{{"error":"{long}"}}"#));
        assert!(
            m.chars().count() <= MAX_ERROR_CHARS,
            "{}",
            m.chars().count()
        );
        assert!(!m.contains('\n'));
    }
}
