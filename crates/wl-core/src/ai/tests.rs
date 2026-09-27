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

/// Wraps a plan the way the preview does, for the tests that exercise
/// `persist_plan` directly and never went through a fetch.
fn preview_of(plan: &PlanResult) -> PlanPreview {
    PlanPreview {
        plan: plan.clone(),
        repair: RepairReport::default(),
        complexity: DEFAULT_COMPLEXITY,
        fallback: None,
    }
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

/// The schema is enforced in two steps — `parse_plan` deserializes,
/// `validate_plan` judges — because the repair pass has to see a plan it
/// intends to fix. Every "the plan is rejected" assertion in this file is
/// therefore on the pair, and these are the ones that would be easiest to
/// accidentally weaken.
#[track_caller]
fn rejects(raw: &str) -> bool {
    match parse_plan(raw) {
        Err(_) => true,
        Ok(plan) => validate_plan(&plan).is_err(),
    }
}

#[test]
fn an_empty_or_oversized_plan_is_rejected() {
    assert!(rejects(r#"{"milestones":[]}"#));
    let six: String = (0..6)
        .map(|i| format!(r#"{{"title":"m{i}","directives":[]}}"#))
        .collect::<Vec<_>>()
        .join(",");
    assert!(rejects(&format!("{{\"milestones\":[{six}]}}")));
}

#[test]
fn a_long_directive_without_phases_is_rejected() {
    let bad = r#"{"milestones":[{"title":"M","directives":[
      {"title":"Do everything","estimated_minutes":90,"phases":[]}]}]}"#;
    assert!(rejects(bad));
}

#[test]
fn out_of_band_phase_minutes_are_rejected() {
    let bad = r#"{"milestones":[{"title":"M","directives":[
      {"title":"X","estimated_minutes":60,"phases":[{"title":"p","minutes":90}]}]}]}"#;
    assert!(rejects(bad));
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
    let mut first_goal: Option<String> = None;
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
            None,
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
        // Scoped to this goal: the loop below runs three providers, so a
        // bare `SUM` over the table is three plans' worth.
        first_goal = Some(persisted.goal_id);
    }
    let first_goal = first_goal.expect("the loop ran at least once");
    // 3 directives per plan x 3 providers.
    let count: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM directives", [], |x| x.get(0))
        .unwrap();
    assert_eq!(count, 9);
    // The 50-min directive is progressive, and its phase table now sums to
    // the ESTIMATE rather than to 30. That is the repair pass (2026-09-27):
    // the old behaviour left 5+25 under a 50-minute headline, and
    // `persist_plan` let the sum override the headline anyway — so the
    // preview, the badge and the stored row all disagreed with each other.
    // Reconciling produces 8 + 21 + 21: the 42-minute step is over the
    // 25-minute band, so it is split rather than clamped, which is the
    // difference between keeping the time and silently eating it.
    let prog = r
        .conn.lock().unwrap()
        .query_row(
            "SELECT progressive_total, estimated_minutes FROM directives WHERE progressive_total > 1",
            [],
            |x| Ok((x.get::<_, i64>(0)?, x.get::<_, i64>(1)?)),
        )
        .unwrap();
    let (total, mins) = prog;
    assert_eq!(total, 3, "the 42-minute step was split, not clamped");
    assert_eq!(mins, 50, "the steps sum to the estimate the model gave");
    let phase_sum: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT SUM(p.minutes) FROM directive_phases p
             JOIN directives d ON d.id = p.directive_id
             JOIN milestones m ON m.id = d.milestone_id
             WHERE m.goal_id = ?1 AND d.progressive_total > 1",
            [&first_goal],
            |x| x.get(0),
        )
        .unwrap();
    assert_eq!(
        phase_sum, 50,
        "the badge, the phase table and the row must agree"
    );
    let widest: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT MAX(p.minutes) FROM directive_phases p
             JOIN directives d ON d.id = p.directive_id
             JOIN milestones m ON m.id = d.milestone_id
             WHERE m.goal_id = ?1 AND d.progressive_total > 1",
            [&first_goal],
            |x| x.get(0),
        )
        .unwrap();
    assert!(
        (1..=25).contains(&widest),
        "every step must sit in the 5-25 band, widest was {widest}"
    );
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
        None,
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
        None,
        execute,
        &r,
        None,
    )
    .unwrap();
    // The repair pass FILLS the title in from the intent, so what comes
    // back is no longer `None`. That is the change: the goal is named
    // either way, and the preview the user is about to see carries the
    // name it will be stored under rather than a gap that `persist_plan`
    // fills in silently behind the user's back.
    assert_eq!(plan.title.as_deref(), Some("in n out burger"));
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
    let (persisted, _) = AiDispatcher::master_plan(
        &provider,
        "in n out burger",
        None,
        DEFAULT_COMPLEXITY,
        None,
        execute,
        &r,
        None,
    )
    .unwrap();
    assert_eq!(
        r.goal(&persisted.goal_id).unwrap().unwrap().title,
        "in n out burger"
    );
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
    let (persisted, _) = AiDispatcher::master_plan(
        &provider,
        &long,
        None,
        DEFAULT_COMPLEXITY,
        None,
        execute,
        &r,
        None,
    )
    .unwrap();
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
    let out = AiDispatcher::master_plan(
        &provider,
        "G",
        None,
        DEFAULT_COMPLEXITY,
        None,
        execute,
        &r,
        None,
    );
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
    assert!(rejects(bad));
    // Within the 2x band: accepted.
    let ok = r#"{"milestones":[{"title":"M","directives":[
      {"title":"Big migration","estimated_minutes":60,"phases":[
        {"title":"Step one","minutes":25},
        {"title":"Step two","minutes":25}]}]}]}"#;
    assert!(!rejects(ok));
}

#[test]
fn validation_bounds_and_persistence_preflight() {
    let r = repos();
    let mut plan = parse_plan(PLAN_JSON).unwrap();
    for minutes in [i64::MIN, 0, i64::MAX] {
        plan.milestones[0].directives[1].phases[0].minutes = minutes;
        assert!(persist_plan(&r, None, &preview_of(&plan), None).is_err());
        assert!(rejects(&serde_json::to_string(&plan).unwrap()));
        assert!(r.active_goal().unwrap().is_none());
    }
    let mut plan = parse_plan(PLAN_JSON).unwrap();
    plan.milestones[0].directives[1].estimated_minutes = i64::MAX;
    assert!(persist_plan(&r, None, &preview_of(&plan), None).is_err());
    for count in [1, 5] {
        let mut plan = parse_plan(PLAN_JSON).unwrap();
        let d = &mut plan.milestones[0].directives[1];
        d.phases = vec![d.phases[0].clone(); count];
        assert!(persist_plan(&r, None, &preview_of(&plan), None).is_err());
    }
    let mut plan = parse_plan(PLAN_JSON).unwrap();
    plan.milestones[0].directives = vec![plan.milestones[0].directives[0].clone(); 33];
    assert!(persist_plan(&r, None, &preview_of(&plan), None).is_err());
    let mut plan = parse_plan(PLAN_JSON).unwrap();
    plan.milestones[1].description = Some("x".repeat(16 * 1024 + 1));
    assert!(persist_plan(&r, None, &preview_of(&plan), None).is_err());
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
    let result = AiDispatcher::master_plan(
        &provider,
        "Goal",
        None,
        DEFAULT_COMPLEXITY,
        None,
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
    assert!(AiDispatcher::master_plan(
        &provider,
        "  ",
        None,
        DEFAULT_COMPLEXITY,
        None,
        no_http,
        &r,
        None
    )
    .is_err());
    assert!(AiDispatcher::master_plan(
        &provider,
        "Goal",
        Some("next Friday"),
        DEFAULT_COMPLEXITY,
        None,
        no_http,
        &r,
        None
    )
    .is_err());
    // The intent is bounded by the description budget, not the old 16 KiB
    // context budget, so one char over 4000 must be refused.
    assert!(AiDispatcher::master_plan(
        &provider,
        &"c".repeat(crate::domain::MAX_DESCRIPTION_CHARS + 1),
        None,
        DEFAULT_COMPLEXITY,
        None,
        no_http,
        &r,
        None
    )
    .is_err());
}

// ---------------------------------------------------------------------------
// The hang: transport errors, the repair pass, and the staged preview.
// 2026-09-27 — "I click generate and it just says planning".
// ---------------------------------------------------------------------------

/// A request that never reaches the provider must say so, and must not
/// blame the response.
///
/// This is the test that was missing when the bug shipped. A timeout used
/// to arrive as `DispatchError::BadJson`, so the user was told "the
/// architect's reply was not usable JSON: error sending request for url
/// (https://…)" — a complaint about a reply that never arrived. The two
/// variants are now distinct and the message names the provider, because
/// "could not reach openrouter" is the one thing the user can act on.
#[test]
fn a_transport_failure_names_the_provider_and_does_not_claim_bad_json() {
    let provider = ProviderAdapter::OpenAiCompat {
        base_url: "https://openrouter.ai/api/v1".into(),
        api_key: Zeroizing::new("k".into()),
        model: "m".into(),
        json_mode: true,
    };
    let err = AiDispatcher::master_plan_preview(
        &provider,
        "in n out burger",
        None,
        DEFAULT_COMPLEXITY,
        None,
        |_, _, _| Err("operation timed out after 120s".to_string()),
    )
    .expect_err("a timeout is a failure");
    assert!(
        matches!(err, DispatchError::Transport { .. }),
        "a transport failure must not be reported as a parse failure: {err:?}"
    );
    let msg = err.to_string();
    assert!(msg.contains("openrouter"), "must name the provider: {msg}");
    assert!(msg.contains("timed out"), "must keep the cause: {msg}");
    assert!(
        !msg.contains("JSON"),
        "must not blame the reply for a network problem: {msg}"
    );
}

/// A reply that really is unreadable is still a `BadJson` — the split must
/// not have swallowed the other direction.
#[test]
fn an_unreadable_reply_is_still_reported_as_bad_json() {
    let provider = ProviderAdapter::OpenAiCompat {
        base_url: "https://openrouter.ai/api/v1".into(),
        api_key: Zeroizing::new("k".into()),
        model: "m".into(),
        json_mode: true,
    };
    let err = AiDispatcher::master_plan_preview(
        &provider,
        "in n out burger",
        None,
        DEFAULT_COMPLEXITY,
        None,
        |_, _, _| {
            Ok::<String, String>(
                r#"{"choices":[{"message":{"content":"here you go: a plan"}}]}"#.into(),
            )
        },
    )
    .expect_err("prose instead of JSON is a failure");
    assert!(
        matches!(err, DispatchError::BadJson(_)),
        "a genuinely unreadable reply is a parse failure: {err:?}"
    );
}

/// The completion is bounded. No `max_tokens` left a provider free to run
/// for minutes, which is the mechanism behind a two-minute "Planning…".
#[test]
fn the_request_bounds_its_own_completion() {
    let provider = ProviderAdapter::OpenAiCompat {
        base_url: "https://openrouter.ai/api/v1".into(),
        api_key: Zeroizing::new("k".into()),
        model: "m".into(),
        json_mode: true,
    };
    let (_, _, body) = provider.request("sys", "user");
    assert_eq!(
        body["max_tokens"], MAX_COMPLETION_TOKENS,
        "an unbounded completion is how a plan request becomes a two-minute wait"
    );
    // The ceiling has to clear a real plan, or every long one is truncated
    // into invalid JSON. Enforced at compile time like the other bounds in
    // this codebase, not by a test that only fails after a build.
    const _: () = assert!(
        MAX_COMPLETION_TOKENS >= 1024,
        "the completion ceiling must clear a real plan"
    );
    const _: () = assert!(
        MAX_COMPLETION_TOKENS <= 32_768,
        "and must not be so high that a plan request can run for minutes"
    );
}

// -- the repair pass ------------------------------------------------------

fn plan_from(json: &str) -> PlanResult {
    parse_plan(json).unwrap()
}

fn repaired(json: &str) -> (PlanResult, RepairReport) {
    let mut p = plan_from(json);
    let r = repair_plan(&mut p, "in n out burger");
    (p, r)
}

/// The headline: a plan that violates the schema in a way the old
/// all-or-nothing validator rejected outright is now repaired and returned.
/// Each of these is a case that used to cost the user their entire goal.
#[test]
fn a_plan_the_old_validator_rejected_is_repaired_not_thrown_away() {
    // A 90-minute task with no phases. The old `validate_directive`
    // returned `Invalid("90min directive requires progressive phases")`
    // and the user got nothing.
    let (p, report) = repaired(
        r#"{"milestones":[{"title":"M","directives":[
            {"title":"Write the whole thing","estimated_minutes":90,"phases":[]}]}]}"#,
    );
    assert!(
        validate_plan(&p).is_ok(),
        "repaired: {:?}",
        validate_plan(&p)
    );
    let d = &p.milestones[0].directives[0];
    assert_eq!(
        d.phases.len(),
        4,
        "90 minutes needs four 22-ish steps, not two oversized ones"
    );
    assert!(d.phases.iter().all(|x| (1..=25).contains(&x.minutes)));
    let sum: i64 = d.phases.iter().map(|x| x.minutes).sum();
    assert_eq!(sum, d.estimated_minutes, "steps must sum to the estimate");
    assert!(!report.is_empty(), "the user is told what changed");
}

#[test]
fn absurd_estimates_and_empty_titles_are_clamped_and_named() {
    let (p, report) = repaired(
        r#"{"milestones":[{"title":"","directives":[
            {"title":"","estimated_minutes":99999,"phases":[]},
            {"title":"Sane","estimated_minutes":20,"phases":[]}]}]}"#,
    );
    let m = &p.milestones[0];
    assert!(!m.title.trim().is_empty(), "a blank milestone gets a name");
    assert!(!m.directives[0].title.trim().is_empty(), "so does a task");
    // 99 999 minutes cannot be four 25-minute steps, so the task is capped
    // at 100 and the note says why. The alternative the old validator took
    // was to throw the whole plan away.
    assert_eq!(m.directives[0].estimated_minutes, 100);
    assert_eq!(m.directives[0].phases.len(), 4);
    assert!(report.notes.iter().any(|n| n.contains("several tasks")));
    assert!(!report.is_empty());
    assert!(
        validate_plan(&p).is_ok(),
        "repaired: {:?}",
        validate_plan(&p)
    );
}

#[test]
fn a_plan_with_more_than_five_milestones_is_truncated_not_rejected() {
    let ms: Vec<String> = (0..8)
        .map(|i| format!(r#"{{"title":"M{i}","directives":[{{"title":"t","estimated_minutes":20,"phases":[]}}]}}"#))
        .collect();
    let json = format!(r#"{{"milestones":[{}]}}"#, ms.join(","));
    let (p, report) = repaired(&json);
    assert_eq!(p.milestones.len(), MAX_MILESTONES);
    assert!(report
        .notes
        .iter()
        .any(|n| n.contains("milestones were kept")));
    assert!(validate_plan(&p).is_ok());
}

#[test]
fn a_milestone_with_no_usable_tasks_is_dropped_and_the_rest_survive() {
    let (p, _) = repaired(
        r#"{"milestones":[
            {"title":"Empty","directives":[]},
            {"title":"Real","directives":[{"title":"t","estimated_minutes":20,"phases":[]}]}]}"#,
    );
    assert_eq!(p.milestones.len(), 1);
    assert_eq!(p.milestones[0].title, "Real");
    assert!(validate_plan(&p).is_ok());
}

#[test]
fn phases_with_impossible_times_are_dropped_and_the_rest_rescaled() {
    let (p, _) = repaired(
        r#"{"milestones":[{"title":"M","directives":[
            {"title":"t","estimated_minutes":40,"phases":[
                {"title":"a","minutes":-5},{"title":"b","minutes":20},{"title":"c","minutes":0}]}]}]}"#,
    );
    let d = &p.milestones[0].directives[0];
    // A negative and a zero step are not steps. One survives, absorbing the
    // whole 40-minute estimate — which is then over the 25-minute band, so
    // it is SPLIT rather than clamped. Two is the right answer: clamping
    // would silently lose 15 minutes of work the user was told about.
    assert_eq!(
        d.phases.len(),
        2,
        "the one surviving step is over the band, so it is split"
    );
    let sum: i64 = d.phases.iter().map(|x| x.minutes).sum();
    assert_eq!(sum, d.estimated_minutes, "steps must sum to the estimate");
    assert!(d.phases.iter().all(|x| (1..=25).contains(&x.minutes)));
    assert!(validate_plan(&p).is_ok());
}

/// A forward or self `after` is a bad index, not a bad plan. Clearing it
/// is what makes the PREVIEW match what gets written — the whole point of
/// showing a preview is that it is not a promise.
#[test]
fn a_forward_or_self_edge_is_cleared_rather_than_kept() {
    let (p, _) = repaired(
        r#"{"milestones":[{"title":"M","directives":[
            {"title":"first","estimated_minutes":20,"phases":[],"after":5},
            {"title":"second","estimated_minutes":20,"phases":[],"after":0},
            {"title":"third","estimated_minutes":20,"phases":[],"after":2}]}]}"#,
    );
    let ds = &p.milestones[0].directives;
    assert_eq!(ds[0].after, None, "a forward reference is dropped");
    assert_eq!(ds[1].after, Some(0), "a backward reference is kept");
    assert_eq!(ds[2].after, None, "a self reference is dropped");
    assert!(validate_plan(&p).is_ok());
}

/// The repair pass is deterministic: same JSON in, same plan out. A repair
/// that ran differently on two devices would make the preview and the
/// stored row disagree, which is the failure this whole pass exists to stop.
#[test]
fn repair_is_deterministic() {
    let json = r#"{"milestones":[{"title":"M","directives":[
        {"title":"long","estimated_minutes":137,"phases":[]},
        {"title":"after","estimated_minutes":20,"phases":[],"after":0}]}]}"#;
    let (a, ra) = repaired(json);
    let (b, rb) = repaired(json);
    assert_eq!(a, b);
    assert_eq!(ra, rb);
}

// -- the staged preview ---------------------------------------------------

#[test]
fn a_preview_writes_nothing_at_all() {
    let r = repos();
    let provider = ProviderAdapter::OpenAiCompat {
        base_url: "https://openrouter.ai/api/v1".into(),
        api_key: Zeroizing::new("k".into()),
        model: "m".into(),
        json_mode: true,
    };
    let preview = AiDispatcher::master_plan_preview(
        &provider,
        "Ship the relay",
        Some("2026-12-31"),
        DEFAULT_COMPLEXITY,
        None,
        |_, _, _| {
            Ok::<String, String>(
                serde_json::json!({"choices":[{"message":{"content": PLAN_JSON}}]}).to_string(),
            )
        },
    )
    .unwrap();
    assert!(!preview.is_fallback());
    assert_eq!(preview.complexity, DEFAULT_COMPLEXITY);
    // The load-bearing assertion: fetching a plan created no goal, no
    // milestone and no task. Backing out of the preview has to be free.
    let goals: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM goals", [], |x| x.get(0))
        .unwrap();
    let tasks: i64 = r
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM directives", [], |x| x.get(0))
        .unwrap();
    assert_eq!((goals, tasks), (0, 0), "a preview must not write");
}

#[test]
fn a_plan_with_nothing_in_it_becomes_an_editable_fallback_not_an_error() {
    let provider = ProviderAdapter::OpenAiCompat {
        base_url: "https://openrouter.ai/api/v1".into(),
        api_key: Zeroizing::new("k".into()),
        model: "m".into(),
        json_mode: true,
    };
    let preview = AiDispatcher::master_plan_preview(
        &provider,
        "\n  in n out burger  \nmore rambling",
        None,
        4,
        None,
        |_, _, _| {
            Ok::<String, String>(
                serde_json::json!({"choices":[{"message":{"content":"{\"milestones\":[]}"}}]})
                    .to_string(),
            )
        },
    )
    .unwrap();
    assert!(preview.is_fallback());
    assert!(
        preview.fallback.as_deref().unwrap().contains("first step"),
        "the reason is shown, not silently swapped: {:?}",
        preview.fallback
    );
    // The stand-in is a real, storable, editable plan: one task, named from
    // the user's own words, under the threshold so it needs no phases.
    assert_eq!(preview.complexity, 4, "the rating survives the fallback");
    assert!(validate_plan(&preview.plan).is_ok());
    assert_eq!(preview.plan.milestones.len(), 1);
    let d = &preview.plan.milestones[0].directives[0];
    assert_eq!(d.title, "in n out burger");
    assert_eq!(d.estimated_minutes, 25);
    assert!(d.phases.is_empty());
    assert_eq!(preview.plan.title.as_deref(), Some("in n out burger"));
}

/// The prompt has to carry BOTH the rating and the record, or the
/// estimator has no consumer and the number is decoration.
#[test]
fn the_prompt_carries_the_rating_and_the_record() {
    let bare = crate::ai::prompt::tier1_user("ship it", None, 5, None);
    assert!(bare.contains("5/5"), "the rating must be in the prompt");
    assert!(
        bare.contains("SIZE THE PLAN"),
        "and its effect spelled out, or a model treats the rating as licence to sprawl"
    );
    let with_record = crate::ai::prompt::tier1_user(
        "ship it",
        None,
        5,
        Some("The user rates this deep. Historically they finish 2 of 9 of that kind of work (72% expected; the rating alone said 75%)."),
    );
    assert!(with_record.contains("2 of 9"), "{with_record}");
    assert!(
        with_record.contains("rating alone said 75"),
        "{with_record}"
    );
    // The edge contract is stated, or the model will not emit `after`.
    assert!(bare.contains("after"), "{bare}");
    assert!(
        bare.contains("ONLY where one task genuinely"),
        "an edge on everything is an unreadable graph: {bare}"
    );
}
