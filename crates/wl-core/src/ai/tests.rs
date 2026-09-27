//! AI dispatch tests: strict JSON parsing, schema validation, prompt
//! construction, and persistence. No network calls — the HTTP hop is
//! always injected.

use super::dispatch::*;
use crate::store::open_in_memory;
use crate::store::repo::Repos;
use zeroize::Zeroizing;

fn repos() -> Repos {
    Repos::new(open_in_memory().unwrap(), 1)
}

/// The rating the compose screen sends when nobody touches the slider.
const DEFAULT_COMPLEXITY: i64 = crate::domain::COMPLEXITY_DEFAULT;

const PLAN_JSON: &str = r#"{"milestones":[
  {"title":"Crypto core","description":"Keys and E2EE","directives":[
    {"title":"Write BIP-39 test vectors","estimated_minutes":25,"phases":[]},
    {"title":"Implement ChaCha20 seal/unseal","estimated_minutes":50,"phases":[
      {"title":"Write function signatures","instruction":"5 min","minutes":5},
      {"title":"Implement core logic","minutes":25}
    ]}
  ]},
  {"title":"Directive canvas","directives":[
    {"title":"Build HUD component","estimated_minutes":30,"phases":[]}
  ]}
]}"#;

/// The same plan, but the architect named the goal — which is the normal
/// case now that the compose screen collects raw intent rather than a
/// title.
const PLAN_JSON_TITLED: &str = r#"{"title":"Ship Worldline v0.1",
  "description":"A working 9:16 execution terminal.",
  "milestones":[
  {"title":"Crypto core","description":"Keys and E2EE","directives":[
    {"title":"Write BIP-39 test vectors","estimated_minutes":25,"phases":[]}
  ]}
]}"#;

#[test]
fn parse_plan_valid() {
    let p = parse_plan(PLAN_JSON).unwrap();
    assert_eq!(p.milestones.len(), 2);
    assert_eq!(p.milestones[0].directives.len(), 2);
    assert_eq!(p.milestones[0].directives[1].phases.len(), 2);
}

#[test]
fn parse_plan_strips_fences_and_prose() {
    let fenced = format!("Here is your plan:\n```json\n{PLAN_JSON}\n```\nGood luck!");
    let p = parse_plan(&fenced).unwrap();
    assert_eq!(p.milestones.len(), 2);
}

#[test]
fn parse_plan_rejects_empty_and_huge() {
    assert!(parse_plan(r#"{"milestones":[]}"#).is_err());
    let six: String = (0..6)
        .map(|i| format!(r#"{{"title":"m{i}","directives":[]}}"#))
        .collect::<Vec<_>>()
        .join(",");
    assert!(parse_plan(&format!("{{\"milestones\":[{six}]}}")).is_err());
}

#[test]
fn parse_plan_rejects_long_directive_without_phases() {
    let bad = r#"{"milestones":[{"title":"M","directives":[
      {"title":"Do everything","estimated_minutes":90,"phases":[]}]}]}"#;
    assert!(parse_plan(bad).is_err());
}

#[test]
fn parse_plan_rejects_out_of_band_phase_minutes() {
    let bad = r#"{"milestones":[{"title":"M","directives":[
      {"title":"X","estimated_minutes":60,"phases":[{"title":"p","minutes":90}]}]}]}"#;
    assert!(parse_plan(bad).is_err());
}

#[test]
fn provider_request_shapes() {
    // One OpenAI-compatible shape serves all three approved providers
    // (openrouter | google | bytez.com) — only the base URL varies.
    for base in [
        "https://openrouter.ai/api/v1",
        "https://generativelanguage.googleapis.com/v1beta/openai",
        "https://api.bytez.com/models/v2/openai/v1",
    ] {
        let oa = ProviderAdapter::OpenAiCompat {
            base_url: base.into(),
            api_key: Zeroizing::new("sk-test".into()),
            model: "user-model-x".into(),
            json_mode: true,
        };
        let (url, headers, body) = oa.request("sys", "usr");
        assert_eq!(url, format!("{base}/chat/completions"));
        assert!(headers
            .iter()
            .any(|(k, v)| k == "Authorization" && v == "Bearer sk-test"));
        assert_eq!(body["model"], "user-model-x");
        assert_eq!(body["response_format"]["type"], "json_object");
        let text = oa.extract_text(&serde_json::json!({"choices":[{"message":{"content":"hi"}}]}));
        assert_eq!(text.as_deref(), Some("hi"));
    }
}

/// `response_format` is per-model. Sending it to a model that does not
/// advertise it is a 400, and the catalog is the only thing that knows
/// which models those are — so the flag has to actually reach the body.
#[test]
fn json_mode_off_omits_response_format() {
    let mk = |json_mode| ProviderAdapter::OpenAiCompat {
        base_url: "https://openrouter.ai/api/v1".into(),
        api_key: Zeroizing::new("k".into()),
        model: "anthropic/claude-sonnet-4".into(),
        json_mode,
    };
    let (_, _, on) = mk(true).request("sys", "usr");
    assert_eq!(on["response_format"]["type"], "json_object");
    let (_, _, off) = mk(false).request("sys", "usr");
    assert!(
        off.get("response_format").is_none(),
        "an unsupported field must be absent, not null: {off}"
    );
    // Everything else about the request is unchanged.
    assert_eq!(off["model"], "anthropic/claude-sonnet-4");
    assert_eq!(off["messages"][0]["role"], "system");
    assert_eq!(off["temperature"], 0.4);
}

#[test]
fn master_plan_end_to_end_with_injected_http() {
    let r = repos();
    // B-001 regression: master_plan with mocked `execute` succeeds for
    // each approved provider (base URL is the only variance).
    for base in [
        "https://openrouter.ai/api/v1",
        "https://generativelanguage.googleapis.com/v1beta/openai",
        "https://api.bytez.com/models/v2/openai/v1",
    ] {
        let provider = ProviderAdapter::OpenAiCompat {
            base_url: base.into(),
            api_key: Zeroizing::new("k".into()),
            model: "m".into(),
            json_mode: true,
        };
        let execute = |_: &str, _: &[(String, String)], _: &serde_json::Value| {
            Ok::<String, String>(
                serde_json::json!({"choices":[{"message":{"content": PLAN_JSON}}]}).to_string(),
            )
        };
        let (persisted, plan) = AiDispatcher::master_plan(
            &provider,
            "Ship Worldline",
            Some("2026-10-01"),
            DEFAULT_COMPLEXITY,
            execute,
            &r,
            None,
        )
        .unwrap();
        assert_eq!(plan.milestones.len(), 2);
        let g = r.goal(&persisted.goal_id).unwrap().unwrap();
        // `PLAN_JSON` carries no `title`, so the intent is the fallback.
        assert_eq!(g.title, "Ship Worldline");
        let ms = r.milestones_for_goal(&persisted.goal_id).unwrap();
        assert_eq!(ms.len(), 2);
    }
    // 3 directives per plan x 3 providers.
    let count: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM directives", [], |x| x.get(0))
        .unwrap();
    assert_eq!(count, 9);
    // The 50-min directive became progressive with 2 phases and
    // minutes summed from phases.
    let prog = r
        .conn.lock().unwrap()
        .query_row(
            "SELECT progressive_total, estimated_minutes FROM directives WHERE progressive_total > 1",
            [],
            |x| Ok((x.get::<_, i64>(0)?, x.get::<_, i64>(1)?)),
        )
        .unwrap();
    let (total, mins) = prog;
    assert_eq!(total, 2);
    assert_eq!(mins, 30); // 5 + 25
}

/// The compose screen collects raw intent, not a title, so naming the goal
/// is the architect's job. When it offers a title, that title is the goal
/// — the user's fragment must NOT survive as the goal name.
#[test]
fn master_plan_prefers_the_architect_title_over_the_intent() {
    let r = repos();
    let provider = ProviderAdapter::OpenAiCompat {
        base_url: "http://localhost".into(),
        api_key: Zeroizing::new("k".into()),
        model: "m".into(),
        json_mode: true,
    };
    let execute = |_: &str, _: &[(String, String)], _: &serde_json::Value| {
        Ok::<String, String>(
            serde_json::json!({"choices":[{"message":{"content": PLAN_JSON_TITLED}}]}).to_string(),
        )
    };
    let (persisted, plan) = AiDispatcher::master_plan(
        &provider,
        "in n out burger",
        Some("2026-10-01"),
        DEFAULT_COMPLEXITY,
        execute,
        &r,
        None,
    )
    .unwrap();
    assert_eq!(plan.title.as_deref(), Some("Ship Worldline v0.1"));
    let g = r.goal(&persisted.goal_id).unwrap().unwrap();
    assert_eq!(g.title, "Ship Worldline v0.1");
    // The model's description becomes the goal's framing.
    assert_eq!(
        g.description.as_deref(),
        Some("A working 9:16 execution terminal.")
    );
}

/// A model that returns a good plan but no title must not cost the user
/// the request — the intent's first line becomes the title. This is the
/// case that would otherwise silently produce an "Untitled goal".
#[test]
fn master_plan_falls_back_to_the_intent_when_no_title_is_offered() {
    let r = repos();
    let provider = ProviderAdapter::OpenAiCompat {
        base_url: "http://localhost".into(),
        api_key: Zeroizing::new("k".into()),
        model: "m".into(),
        json_mode: true,
    };
    let execute = |_: &str, _: &[(String, String)], _: &serde_json::Value| {
        Ok::<String, String>(
            serde_json::json!({"choices":[{"message":{"content": PLAN_JSON}}]}).to_string(),
        )
    };
    let (persisted, plan) = AiDispatcher::master_plan(
        &provider,
        "\n  in n out burger  \nthe rest of my rambling plan\nmore",
        None,
        DEFAULT_COMPLEXITY,
        execute,
        &r,
        None,
    )
    .unwrap();
    assert!(plan.title.is_none());
    let g = r.goal(&persisted.goal_id).unwrap().unwrap();
    assert_eq!(g.title, "in n out burger");
}

/// A blank-but-present title is treated as "no title offered", not as an
/// error, and never becomes the goal's name.
#[test]
fn a_blank_architect_title_falls_back_rather_than_naming_the_goal_blank() {
    let r = repos();
    let blank = PLAN_JSON_TITLED.replace(r#""title":"Ship Worldline v0.1""#, r#""title":"   ""#);
    let provider = ProviderAdapter::OpenAiCompat {
        base_url: "http://localhost".into(),
        api_key: Zeroizing::new("k".into()),
        model: "m".into(),
        json_mode: true,
    };
    let execute = move |_: &str, _: &[(String, String)], _: &serde_json::Value| {
        Ok::<String, String>(
            serde_json::json!({"choices":[{"message":{"content": blank}}]}).to_string(),
        )
    };
    let (persisted, _) =
        AiDispatcher::master_plan(&provider, "in n out burger", None, DEFAULT_COMPLEXITY, execute, &r, None).unwrap();
    assert_eq!(r.goal(&persisted.goal_id).unwrap().unwrap().title, "in n out burger");
}

/// The intent is prose and can be long. The fallback title must still fit
/// `MAX_TITLE_CHARS`, because `create_goal` enforces that bound and would
/// otherwise reject an otherwise-valid plan over a cosmetic omission.
#[test]
fn a_long_single_line_intent_still_produces_a_valid_title() {
    let r = repos();
    let provider = ProviderAdapter::OpenAiCompat {
        base_url: "http://localhost".into(),
        api_key: Zeroizing::new("k".into()),
        model: "m".into(),
        json_mode: true,
    };
    let execute = |_: &str, _: &[(String, String)], _: &serde_json::Value| {
        Ok::<String, String>(
            serde_json::json!({"choices":[{"message":{"content": PLAN_JSON}}]}).to_string(),
        )
    };
    // One long line, no spaces near the clip point, to prove the word
    // boundary never yields an over-budget or empty title.
    let long = format!("{} {}", "a".repeat(600), "tail");
    let (persisted, _) =
        AiDispatcher::master_plan(&provider, &long, None, DEFAULT_COMPLEXITY, execute, &r, None).unwrap();
    let g = r.goal(&persisted.goal_id).unwrap().unwrap();
    assert!(
        g.title.chars().count() <= crate::domain::MAX_TITLE_CHARS,
        "fallback title was {} chars, over the {} budget",
        g.title.chars().count(),
        crate::domain::MAX_TITLE_CHARS
    );
    assert!(!g.title.is_empty());
}

#[test]
fn fallback_title_uses_the_first_non_empty_line_and_clips_on_a_word() {
    // First non-empty line, trimmed.
    assert_eq!(
        fallback_title("\n\n  in n out burger  \nmore"),
        "in n out burger"
    );
    // Nothing usable at all.
    assert_eq!(fallback_title("   \n\t\n"), "");
    // Under budget: returned whole.
    assert_eq!(fallback_title("ship it"), "ship it");
    // Over budget: clipped at the last space, never mid-word, never empty.
    let long = format!("{} {}", "word ".repeat(200), "end");
    let t = fallback_title(&long);
    assert!(t.chars().count() <= crate::domain::MAX_TITLE_CHARS);
    assert!(t.ends_with("word"));
    // A first "word" longer than the whole budget must not clip to empty.
    let solid = "z".repeat(crate::domain::MAX_TITLE_CHARS + 50);
    let t = fallback_title(&solid);
    assert_eq!(t.chars().count(), crate::domain::MAX_TITLE_CHARS);
}

#[test]
fn http_failure_maps_to_error() {
    let r = repos();
    let provider = ProviderAdapter::OpenAiCompat {
        base_url: "http://x".into(),
        api_key: Zeroizing::new("k".into()),
        model: "m".into(),
        json_mode: true,
    };
    let execute = |_: &str, _: &[(String, String)], _: &serde_json::Value| {
        Err::<String, String>("network down".to_string())
    };
    let out = AiDispatcher::master_plan(&provider, "G", None, DEFAULT_COMPLEXITY, execute, &r, None);
    assert!(out.is_err());
}

#[test]
fn parse_plan_rejects_phases_contradicting_estimate() {
    // The phase table replaces the headline estimate at persist time,
    // so a 60m directive with 10m of phases would silently shrink.
    let bad = r#"{"milestones":[{"title":"M","directives":[
      {"title":"Big migration","estimated_minutes":60,"phases":[
        {"title":"Step one","minutes":5},
        {"title":"Step two","minutes":5}]}]}]}"#;
    assert!(parse_plan(bad).is_err());
    // Within the 2x band: accepted.
    let ok = r#"{"milestones":[{"title":"M","directives":[
      {"title":"Big migration","estimated_minutes":60,"phases":[
        {"title":"Step one","minutes":25},
        {"title":"Step two","minutes":25}]}]}]}"#;
    assert!(parse_plan(ok).is_ok());
}

#[test]
fn validation_bounds_and_persistence_preflight() {
    let r = repos();
    let mut plan = parse_plan(PLAN_JSON).unwrap();
    for minutes in [i64::MIN, 0, i64::MAX] {
        plan.milestones[0].directives[1].phases[0].minutes = minutes;
        assert!(persist_plan(&r, None, &plan, DEFAULT_COMPLEXITY, None).is_err());
        assert!(parse_plan(&serde_json::to_string(&plan).unwrap()).is_err());
        assert!(r.active_goal().unwrap().is_none());
    }
    let mut plan = parse_plan(PLAN_JSON).unwrap();
    plan.milestones[0].directives[1].estimated_minutes = i64::MAX;
    assert!(persist_plan(&r, None, &plan, DEFAULT_COMPLEXITY, None).is_err());
    for count in [1, 5] {
        let mut plan = parse_plan(PLAN_JSON).unwrap();
        let d = &mut plan.milestones[0].directives[1];
        d.phases = vec![d.phases[0].clone(); count];
        assert!(persist_plan(&r, None, &plan, DEFAULT_COMPLEXITY, None).is_err());
    }
    let mut plan = parse_plan(PLAN_JSON).unwrap();
    plan.milestones[0].directives = vec![plan.milestones[0].directives[0].clone(); 33];
    assert!(persist_plan(&r, None, &plan, DEFAULT_COMPLEXITY, None).is_err());
    let mut plan = parse_plan(PLAN_JSON).unwrap();
    plan.milestones[1].description = Some("x".repeat(16 * 1024 + 1));
    assert!(persist_plan(&r, None, &plan, DEFAULT_COMPLEXITY, None).is_err());
    assert!(r.active_goal().unwrap().is_none());
}

#[test]
fn response_size_checked_before_parsing_and_persistence() {
    let raw = " ".repeat(256 * 1024 + 1);
    assert!(matches!(
        parse_plan(&raw),
        Err(DispatchError::Invalid {
            field: "response",
            ..
        })
    ));
    let r = repos();
    let provider = ProviderAdapter::OpenAiCompat {
        base_url: "http://localhost".into(),
        api_key: Zeroizing::new("test".into()),
        model: "test".into(),
        json_mode: true,
    };
    let result =
        AiDispatcher::master_plan(&provider, "Goal", None, DEFAULT_COMPLEXITY, |_, _, _| Ok(raw.clone()), &r, None);
    assert!(matches!(
        result,
        Err(DispatchError::Invalid {
            field: "response",
            ..
        })
    ));
    assert!(r.active_goal().unwrap().is_none());
}

#[test]
fn provider_debug_redacts_api_key() {
    // A derived Debug would print key material into any {:?} path.
    let oa = ProviderAdapter::OpenAiCompat {
        base_url: "https://x".into(),
        api_key: Zeroizing::new("sk-live-secret".into()),
        model: "m".into(),
        json_mode: true,
    };
    let dbg = format!("{oa:?}");
    assert!(!dbg.contains("sk-live-secret"), "key leaked: {dbg}");
    assert!(dbg.contains("[redacted]"));
}

#[test]
fn facades_reject_bad_input_before_any_http_call() {
    let r = repos();
    let provider = ProviderAdapter::OpenAiCompat {
        base_url: "http://localhost".into(),
        api_key: Zeroizing::new("k".into()),
        model: "m".into(),
        json_mode: true,
    };
    let no_http = |_: &str, _: &[(String, String)], _: &serde_json::Value| {
        panic!("HTTP must not be attempted for invalid input")
    };
    // master_plan: blank intent, bad date, over-budget intent.
    assert!(AiDispatcher::master_plan(&provider, "  ", None, DEFAULT_COMPLEXITY, no_http, &r, None).is_err());
    assert!(
        AiDispatcher::master_plan(&provider, "Goal", Some("next Friday"), DEFAULT_COMPLEXITY, no_http, &r, None)
            .is_err()
    );
    // The intent is bounded by the description budget, not the old 16 KiB
    // context budget, so one char over 4000 must be refused.
    assert!(AiDispatcher::master_plan(
        &provider,
        &"c".repeat(crate::domain::MAX_DESCRIPTION_CHARS + 1),
        None,
        DEFAULT_COMPLEXITY,
        no_http,
        &r,
        None
    )
    .is_err());
}
