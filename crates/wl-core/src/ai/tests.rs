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
fn parse_briefing_valid_and_bounded() {
    let ok = r#"{"directives":[{"title":"Write 300 words on Section 2.1","estimated_minutes":45,"phases":[
        {"title":"Open IDE, signature","minutes":5},{"title":"Core loop","minutes":25}]}]}"#;
    let b = parse_briefing(ok).unwrap();
    assert_eq!(b.directives.len(), 1);
    assert!(parse_briefing(r#"{"directives":[]}"#).is_err());
    let four: String = (0..4)
        .map(|i| format!(r#"{{"title":"d{i}","estimated_minutes":10,"phases":[]}}"#))
        .collect::<Vec<_>>()
        .join(",");
    assert!(parse_briefing(&format!("{{\"directives\":[{four}]}}")).is_err());
}

#[test]
fn provider_request_shapes() {
    let oa = ProviderAdapter::OpenAiCompat {
        base_url: "https://openrouter.ai/api/v1".into(),
        api_key: Zeroizing::new("sk-test".into()),
        model: "user-model-x".into(),
    };
    let (url, headers, body) = oa.request("sys", "usr");
    assert!(url.ends_with("/chat/completions"));
    assert!(headers
        .iter()
        .any(|(k, v)| k == "Authorization" && v == "Bearer sk-test"));
    assert_eq!(body["model"], "user-model-x");
    assert_eq!(body["response_format"]["type"], "json_object");
    let text = oa.extract_text(&serde_json::json!({"choices":[{"message":{"content":"hi"}}]}));
    assert_eq!(text.as_deref(), Some("hi"));

    let an = ProviderAdapter::Anthropic {
        api_key: Zeroizing::new("ak".into()),
        model: "claude-user".into(),
    };
    let (url, headers, body) = an.request("sys", "usr");
    assert_eq!(url, "https://api.anthropic.com/v1/messages");
    assert!(headers.iter().any(|(k, v)| k == "x-api-key" && v == "ak"));
    assert_eq!(body["model"], "claude-user");
    let text = an.extract_text(&serde_json::json!({"content":[{"text":"yo"}]}));
    assert_eq!(text.as_deref(), Some("yo"));
}

#[test]
fn master_plan_end_to_end_with_injected_http() {
    let r = repos();
    let provider = ProviderAdapter::Anthropic {
        api_key: Zeroizing::new("k".into()),
        model: "m".into(),
    };
    let execute = |_: &str, _: &[(String, String)], _: &serde_json::Value| {
        Ok::<String, String>(serde_json::json!({"content":[{"text": PLAN_JSON}]}).to_string())
    };
    let (goal_id, plan) = AiDispatcher::master_plan(
        &provider,
        "Ship Worldline",
        Some("v0.1"),
        Some("2026-10-01"),
        "Rust dev",
        execute,
        &r,
        None,
    )
    .unwrap();
    assert_eq!(plan.milestones.len(), 2);
    let g = r.goal(&goal_id).unwrap().unwrap();
    assert_eq!(g.title, "Ship Worldline");
    let ms = r.milestones_for_goal(&goal_id).unwrap();
    assert_eq!(ms.len(), 2);
    // 3 directives total across milestones.
    let count: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM directives", [], |x| x.get(0))
        .unwrap();
    assert_eq!(count, 3);
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

#[test]
fn morning_briefing_persists_into_next_milestone() {
    let r = repos();
    // Seed goal + milestone manually (manual fallback path).
    let g = r.create_goal("Ship", None, None, None).unwrap();
    r.create_milestone(&g.id, "M1", None, 0, None).unwrap();
    let provider = ProviderAdapter::OpenAiCompat {
        base_url: "http://localhost".into(),
        api_key: Zeroizing::new("k".into()),
        model: "m".into(),
    };
    let brief_json = r#"{"directives":[{"title":"Write 300 words on Section 2.1","estimated_minutes":45,"phases":[
        {"title":"Signature","minutes":5},{"title":"Core loop","minutes":25}]}]}"#;
    let execute = |_: &str, _: &[(String, String)], _: &serde_json::Value| {
        Ok::<String, String>(
            serde_json::json!({"choices":[{"message":{"content": brief_json}}]}).to_string(),
        )
    };
    let brief = AiDispatcher::morning_briefing(
        &provider,
        "2026-09-13",
        "2h available",
        "{}",
        execute,
        &r,
        None,
    )
    .unwrap();
    assert_eq!(brief.directives.len(), 1);
    let runnable = r.runnable_directives("2026-09-13").unwrap();
    assert_eq!(runnable.len(), 1);
    assert_eq!(runnable[0].scheduled_for_date, "2026-09-13");
    assert_eq!(runnable[0].progressive_total, 2);
}

#[test]
fn http_failure_maps_to_error() {
    let r = repos();
    let provider = ProviderAdapter::OpenAiCompat {
        base_url: "http://x".into(),
        api_key: Zeroizing::new("k".into()),
        model: "m".into(),
    };
    let execute = |_: &str, _: &[(String, String)], _: &serde_json::Value| {
        Err::<String, String>("network down".to_string())
    };
    let out = AiDispatcher::master_plan(&provider, "G", None, None, "", execute, &r, None);
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
        assert!(persist_plan(&r, None, &plan, None).is_err());
        assert!(parse_plan(&serde_json::to_string(&plan).unwrap()).is_err());
        assert!(r.active_goal().unwrap().is_none());
    }
    let mut plan = parse_plan(PLAN_JSON).unwrap();
    plan.milestones[0].directives[1].estimated_minutes = i64::MAX;
    assert!(persist_plan(&r, None, &plan, None).is_err());
    for count in [1, 5] {
        let mut plan = parse_plan(PLAN_JSON).unwrap();
        let d = &mut plan.milestones[0].directives[1];
        d.phases = vec![d.phases[0].clone(); count];
        assert!(persist_plan(&r, None, &plan, None).is_err());
    }
    let mut plan = parse_plan(PLAN_JSON).unwrap();
    plan.milestones[0].directives = vec![plan.milestones[0].directives[0].clone(); 33];
    assert!(persist_plan(&r, None, &plan, None).is_err());
    let mut plan = parse_plan(PLAN_JSON).unwrap();
    plan.milestones[1].description = Some("x".repeat(16 * 1024 + 1));
    assert!(persist_plan(&r, None, &plan, None).is_err());
    assert!(r.active_goal().unwrap().is_none());

    let goal = r.create_goal("Goal", None, None, None).unwrap();
    r.create_milestone(&goal.id, "Milestone", None, 0, None)
        .unwrap();
    let mut brief = BriefingResult {
        directives: parse_plan(PLAN_JSON).unwrap().milestones[0]
            .directives
            .clone(),
    };
    brief.directives[1].title.clear();
    assert!(persist_briefing(&r, &brief, "2026-09-13", None).is_err());
    assert!(r.runnable_directives("2026-09-13").unwrap().is_empty());
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
    assert!(matches!(
        parse_briefing(&raw),
        Err(DispatchError::Invalid {
            field: "response",
            ..
        })
    ));
    let r = repos();
    let provider = ProviderAdapter::Anthropic {
        api_key: Zeroizing::new("test".into()),
        model: "test".into(),
    };
    let result = AiDispatcher::master_plan(
        &provider,
        "Goal",
        None,
        None,
        "",
        |_, _, _| Ok(raw.clone()),
        &r,
        None,
    );
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
    };
    let dbg = format!("{oa:?}");
    assert!(!dbg.contains("sk-live-secret"), "key leaked: {dbg}");
    assert!(dbg.contains("[redacted]"));
}

#[test]
fn facades_reject_bad_input_before_any_http_call() {
    let r = repos();
    let provider = ProviderAdapter::Anthropic {
        api_key: Zeroizing::new("k".into()),
        model: "m".into(),
    };
    let no_http = |_: &str, _: &[(String, String)], _: &serde_json::Value| {
        panic!("HTTP must not be attempted for invalid input")
    };
    // master_plan: blank title, bad date, over-budget context.
    assert!(AiDispatcher::master_plan(&provider, "  ", None, None, "c", no_http, &r, None).is_err());
    assert!(AiDispatcher::master_plan(
        &provider,
        "Goal",
        None,
        Some("next Friday"),
        "c",
        no_http,
        &r,
        None
    )
    .is_err());
    assert!(AiDispatcher::master_plan(
        &provider,
        "Goal",
        None,
        None,
        &"c".repeat(16 * 1024 + 1),
        no_http,
        &r,
        None
    )
    .is_err());
    // morning_briefing: bad date, over-budget constraints.
    assert!(AiDispatcher::morning_briefing(
        &provider,
        "yesterday",
        "c",
        "{}",
        no_http,
        &r,
        None
    )
    .is_err());
    assert!(AiDispatcher::morning_briefing(
        &provider,
        "2026-09-18",
        &"c".repeat(8 * 1024 + 1),
        "{}",
        no_http,
        &r,
        None
    )
    .is_err());
}
