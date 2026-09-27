//! IPC contract tests: the seam between the Dioxus UI's JSON payloads
//! and the command bodies.
//!
//! These tests go through the *real* `generate_handler!` over
//! `tauri::test::MockRuntime` — the same argument binding, the same
//! errors, and the same handler list the app registers (see
//! `add_commands`, which exists so this cannot drift from `main`).
//!
//! The mismatch they pin is quieter than a crash. A *required* parameter
//! whose key does not match fails loudly with "missing required key". An
//! `Option<T>` parameter whose key does not match fails SILENTLY:
//! `CommandItem::deserialize_option` visits `None` for an absent key
//! (`tauri/src/ipc/command.rs:135-140`), so `create_goal` bound
//! `target_date` to nothing and persisted a goal with no deadline while
//! the compose screen's horizon picker looked like it worked. Every test
//! here therefore checks the value that came back, not just that the call
//! did not error.
//!
//! Nothing caught this before: every other test called the Rust functions
//! directly (skipping the IPC layer entirely), and the browser mock in
//! `ui/public/invoke-shim.js` ignores argument shapes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{get_ipc_response, MockRuntime};
use tauri::webview::InvokeRequest;
use tauri::Manager;

use wl_core::domain::today_local;

use crate::add_commands;
use crate::app_state::AppState;
use crate::vault::Vault;

/// Marker in a command's error that means a REQUIRED parameter never
/// reached the function body. Matched on the string because
/// `InvokeError` erases the error type across the IPC boundary.
const UNBOUND: &str = "missing required key";

/// Self-cleaning scratch directory: `Vault::open` writes a snapshot and
/// a device-local key file, and a test that leaks either is a test that
/// litters `$TMPDIR`.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!("wl-ipc-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("scratch dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An `AppState` with a fresh in-memory store and an empty vault: the
/// state a first-run install has, before any identity or key exists.
/// Built field-by-field (not via `AppState::new`) because that needs a
/// real `AppHandle` and therefore a real windowing runtime.
fn fresh_state(scratch: &Scratch) -> Arc<AppState> {
    Arc::new(AppState {
        repos: wl_core::store::repo::Repos::new(wl_core::store::open_in_memory().unwrap(), 1),
        identity: std::sync::Mutex::new(None),
        vault: Vault::open(scratch.path()).expect("vault"),
        relay_token: std::sync::Mutex::new(None),
        catalog: std::sync::Mutex::new(std::collections::HashMap::new()),
    })
}

/// Mock app carrying the production command list.
fn mock_app() -> (tauri::App<MockRuntime>, Scratch) {
    let scratch = Scratch::new("app");
    let state = fresh_state(&scratch);
    let app = add_commands(tauri::test::mock_builder().manage(state))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("mock app");
    tauri::WebviewWindowBuilder::new(&app, "main", tauri::WebviewUrl::default())
        .build()
        .expect("mock webview");
    (app, scratch)
}

/// Invoke `cmd` over the real IPC path, exactly as the webview does.
fn call(
    app: &tauri::App<MockRuntime>,
    cmd: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let webview = app.get_webview_window("main").expect("mock webview");
    get_ipc_response(
        &webview,
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: InvokeBody::Json(args),
            headers: Default::default(),
            invoke_key: tauri::test::INVOKE_KEY.to_string(),
        },
    )
    .map(|body| body.deserialize::<serde_json::Value>().expect("json body"))
    .map_err(|e| e.to_string())
}

/// Asserts a command got past argument binding. A payload whose keys do
/// not match the Rust parameter names fails here with a "missing
/// required key" error instead of reaching the body.
#[track_caller]
fn assert_bound(result: &Result<serde_json::Value, String>, cmd: &str) {
    if let Err(e) = result {
        assert!(
            !e.contains(UNBOUND),
            "IPC contract broken for `{cmd}`: the shell could not bind the \
             payload keys the UI sends.\n  error: {e}\n  This means a command \
             parameter is named differently in commands.rs and in the UI's \
             serde request struct — or a command lost its \
             `rename_all = \"snake_case\"`."
        );
    }
}

/// The manual authoring path, end to end through IPC: the compose screen's
/// `create_goal`, which is the ONLY shell entry point for authoring — it
/// seeds the starter milestone + directive itself (B-002). The
/// `create_manual_milestone` / `create_manual_directive` commands that used
/// to sit behind it had no caller in `ui/src` and were deleted (PRD delta
/// 157); the coverage they carried lives on in the two assertions below.
///
/// Every step asserts a real success, so an unbound parameter is not a
/// silent pass but a hard failure at the first step that uses it.
#[test]
fn manual_authoring_binds_snake_case_payloads_through_ipc() {
    let (app, _scratch) = mock_app();

    // Compose screen → `create_goal(title, description, target_date)`.
    let created = call(
        &app,
        "create_goal",
        serde_json::json!({
            "title": "Ship the relay",
            "description": "Manual goal, no API key.",
            "target_date": "2026-12-31",
            "complexity": 4,
        }),
    )
    .expect("create_goal must bind and succeed");
    assert_bound(&Ok(created.clone()), "create_goal");
    let goal_id = created["id"].as_str().expect("goal id").to_string();
    assert_eq!(created["title"], "Ship the relay");
    // The horizon picker resolves to a real calendar date before it
    // crosses the wire, so the store's shape-exact date check must
    // accept what the UI produced. This IS the silent-loss regression:
    // `target_date` is an `Option`, so a key mismatch binds `None`, the
    // goal persists with no deadline, and nothing anywhere errors.
    assert_eq!(created["target_date"], "2026-12-31");

    let state = app.state::<Arc<AppState>>().inner().clone();
    let goal = state
        .repos
        .goal(&goal_id)
        .expect("goal lookup")
        .expect("manual goal was persisted");
    assert_eq!(goal.target_date.as_deref(), Some("2026-12-31"));
    // The rating is an INTEGER, not an `Option`, so a key mismatch fails
    // loudly here — but it is the one the estimator buckets on, and a goal
    // written at the default when the user chose 4 poisons those numbers
    // permanently. Asserted off the row for the same reason `target_date`
    // is.
    assert_eq!(goal.complexity, 4);

    // The seeded starter set is what the canvas actually renders, so its
    // optional and integer fields are the ones a key mismatch would
    // quietly default: `execution_context` comes from the goal's
    // description, and the estimate is a bare number the UI never sends.
    // Asserted through the row, not `current_directive`, because the
    // seeded directive legitimately owns the canvas for today.
    let seeded = state
        .repos
        .next_runnable_directive(&today_local())
        .expect("runnable lookup")
        .expect("create_goal seeds a runnable directive");
    assert_eq!(seeded.title, "Ship the relay");
    assert_eq!(
        seeded.execution_context.as_deref(),
        Some("Manual goal, no API key.")
    );
    assert_eq!(seeded.estimated_minutes, 25);
}

/// `master_plan(provider, model, intent, target_date)` cannot succeed in
/// a test — there is no BYOK key and no network. It must, however, get
/// far enough to read the vault and report the missing key, which proves
/// the payload bound. The reverse order matters: a key mismatch would
/// surface as "missing required key" long before the vault is touched.
#[test]
fn master_plan_binds_its_payload_before_touching_the_vault() {
    let (app, _scratch) = mock_app();

    let result = call(
        &app,
        "master_plan",
        serde_json::json!({
            "provider": "openrouter",
            "model": "anthropic/claude-sonnet-5",
            "intent": "in n out burger",
            "target_date": serde_json::Value::Null,
        }),
    );
    assert_bound(&result, "master_plan");
    let err = result.expect_err("no API key is configured in this install");
    assert!(
        err.contains("no API key stored for provider openrouter"),
        "master_plan bound its arguments but did not reach the vault: {err}"
    );
}

/// The blank-model refusal must fire BEFORE the vault and before any
/// network call, and it must name the setting to fix rather than
/// reporting "offline".
///
/// This is the regression for the placeholder the compose screen used to
/// send when nothing was configured: `"flagship"`, which is not a model
/// id on any provider, so every AI request 400'd and the UI blamed the
/// network. (The Tier-2 sibling of this test went with the morning
/// briefing; `master_plan` is the only AI command left that takes a
/// model, so it is the only one that can hit this.)
#[test]
fn a_blank_model_is_refused_with_a_pointer_to_settings() {
    let (app, _scratch) = mock_app();

    let result = call(
        &app,
        "master_plan",
        serde_json::json!({
            "provider": "openrouter",
            "model": "",
            "intent": "in n out burger",
            "target_date": serde_json::Value::Null,
        }),
    );
    assert_bound(&result, "master_plan");
    let err = result.expect_err("a blank model must be refused");
    assert!(
        err.contains("no model selected") && err.contains("Settings"),
        "master_plan must refuse a blank model and point at Settings, got: {err}"
    );
}

/// `list_models` is the one command whose argument is entirely optional
/// (`force`), so its `provider` binding is the only thing that can fail
/// — and a mismatch would surface as "unknown AI provider" rather than
/// "missing required key", which is easy to mistake for a real
/// rejection. Pin the binding, and pin that a missing key is reported as
/// a missing key rather than a network failure.
#[test]
fn list_models_binds_its_provider_argument() {
    let (app, _scratch) = mock_app();

    let result = call(
        &app,
        "list_models",
        serde_json::json!({ "provider": "openrouter", "force": false }),
    );
    assert_bound(&result, "list_models");
    let err = result.expect_err("no API key is configured in this install");
    assert!(
        err.contains("no API key stored for provider openrouter"),
        "list_models bound its argument and reached the vault: {err}"
    );

    // `force` omitted entirely: `Option<bool>` must default, not fail.
    let omitted = call(
        &app,
        "list_models",
        serde_json::json!({ "provider": "openrouter" }),
    )
    .expect_err("still no key");
    assert!(
        omitted.contains("no API key stored"),
        "omitting an Option parameter must not break binding: {omitted}"
    );

    // A provider outside the allow-list is refused outright. The error
    // crosses IPC as a JSON string (ShellError serialises with
    // `serialize_str`), hence the quotes in the comparison.
    let rejected = call(
        &app,
        "list_models",
        serde_json::json!({ "provider": "qwen" }),
    )
    .expect_err("qwen was removed");
    assert!(
        rejected.contains("unknown AI provider"),
        "a removed provider must be refused: {rejected}"
    );
}

/// Removing a provider from the allow-list must not brick the settings
/// page.
///
/// `save_settings` rejects an unknown provider, and `settings_save`
/// writes the whole struct — so an install whose `app_settings` still
/// names a since-removed provider would fail *every* save, including a
/// theme toggle, until the user changed the one field causing it. The
/// read path sanitizes instead, which is what this pins.
#[test]
fn a_removed_provider_does_not_wedge_settings() {
    let (app, _scratch) = mock_app();

    // Write the legacy value straight into the store, bypassing the
    // command boundary (which is what an upgrade looks like).
    let state = app.state::<std::sync::Arc<AppState>>();
    let stale = wl_core::domain::AppSettings {
        ai_provider: Some("qwen".into()),
        ..wl_core::domain::AppSettings::default()
    };
    state
        .repos
        .save_settings(&stale, None)
        .expect_err("the write boundary must still reject it");

    // …but a read reports it as unset, and the page keeps working.
    let got = call(&app, "settings_get", serde_json::json!({})).expect("settings_get");
    assert!(
        got["ai_provider"].is_null(),
        "a removed provider must read back as unset: {got}"
    );
    let save = call(
        &app,
        "settings_save",
        serde_json::json!({ "settings": got }),
    );
    assert!(
        save.is_ok(),
        "saving after a provider removal must work: {save:?}"
    );
}

/// Single-word parameters were already correct, so this pins that
/// `rename_all` did not change them: the whole point of the attribute is
/// that it is a no-op for names that need no conversion.
#[test]
fn single_word_arguments_are_unaffected_by_rename_all() {
    let (app, _scratch) = mock_app();

    // `bail_out(reason, note)` — two single-word keys.
    let result = call(
        &app,
        "bail_out",
        serde_json::json!({ "reason": "external_dependency", "note": "blocked on infra" }),
    );
    // A no-active-directive bailout is a legitimate domain error; a key
    // mismatch is not. `assert_bound` separates the two.
    assert_bound(&result, "bail_out");

    let velocity = call(&app, "velocity", serde_json::json!({})).expect("velocity");
    assert!(velocity.is_object(), "velocity must return an object");
}

/// The Control Panel's two read-only pages, over the real handler list:
/// `list_goals` (the goal rail's progress) and `entropy_log` (the
/// bailout history).
///
/// Both take no arguments, so there is no payload to mis-bind here —
/// what can be wrong is the row that crosses back, and it is wrong
/// quietly. `milestone_done`/`milestone_total` are the numbers the goal
/// rail draws, and `reason`/`date`/`still_blocked` are the whole of what
/// the entropy row says: a bailout reason that serialised as a debug
/// variant, or a date read as seconds rather than nanoseconds out of the
/// HLC, would render as a plausible-looking wrong thing. Hence the field
/// NAMES are asserted, not just the values — the UI destructures by
/// name, and a renamed field drops to `undefined` there.
#[test]
fn control_panel_pages_report_progress_and_bailout_history() {
    let (app, _scratch) = mock_app();
    let state = app.state::<Arc<AppState>>().inner().clone();
    let repos = &state.repos;

    // Seeded through `Repos` rather than `create_goal`: this test is
    // about the two read paths, and `create_goal`'s starter milestone
    // would make `milestone_total` 1 forever.
    let goal = repos
        .create_goal("Ship the relay", None, Some("2026-12-31"), None)
        .expect("seed goal");
    let first = repos
        .create_milestone(&goal.id, "Wire the protocol", None, 0, None)
        .expect("milestone 1");
    let second = repos
        .create_milestone(&goal.id, "Ship the binary", None, 1, None)
        .expect("milestone 2");
    repos
        .set_milestone_status(&first.id, wl_core::domain::MilestoneStatus::Completed, None)
        .expect("complete milestone 1");

    // One of the two milestones is done, so the rail's fraction is a
    // real 1-of-2 rather than 0-of-2 or 1-of-1. `assert_bound` is
    // vacuous for a zero-argument command today, but it is the canary
    // for the day one of these grows a parameter: a `target_date`-style
    // key that stops binding is silent here too.
    let goals = call(&app, "list_goals", serde_json::json!({}));
    assert_bound(&goals, "list_goals");
    let goals = goals.expect("list_goals");
    let goal_rows = goals.as_array().expect("array");
    assert_eq!(
        goal_rows.len(),
        1,
        "exactly one goal was seeded, active or not: {goals}"
    );
    let listed = &goal_rows[0];
    let mut keys: Vec<&str> = listed
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "id",
            "milestone_done",
            "milestone_total",
            "target_date",
            "title"
        ],
        "list_goals field names are the UI's destructuring contract: {listed}"
    );
    assert_eq!(listed["id"], goal.id);
    assert_eq!(listed["title"], "Ship the relay");
    assert_eq!(listed["target_date"], "2026-12-31");
    assert_eq!(listed["milestone_done"], 1);
    assert_eq!(listed["milestone_total"], 2);

    let directive = repos
        .create_directive(
            &second.id,
            "Cut the release",
            None,
            25,
            1,
            &today_local(),
            &[],
            None,
        )
        .expect("directive");
    let bailout = repos
        .record_bailout(
            &directive.id,
            wl_core::domain::BailoutReason::ExternalDependency,
            Some("waiting on the relay to come up"),
            None,
        )
        .expect("bailout");
    // ExternalDependency is the one reason that never self-heals, so
    // this is the case `still_blocked` exists to distinguish.
    repos
        .set_directive_state(
            &directive.id,
            wl_core::domain::DirectiveState::Blocked,
            None,
        )
        .expect("block the directive");

    let log = call(&app, "entropy_log", serde_json::json!({}));
    assert_bound(&log, "entropy_log");
    let log = log.expect("entropy_log");
    let entries = log.as_array().expect("array");
    assert_eq!(entries.len(), 1, "one bailout was recorded: {log}");
    let entry = &entries[0];
    let mut keys: Vec<&str> = entry
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "date",
            "directive_id",
            "directive_title",
            "goal_id",
            "goal_title",
            "id",
            "note",
            "reason",
            "still_blocked",
        ],
        "entropy_log field names are the UI's destructuring contract: {entry}"
    );
    assert_eq!(entry["id"], bailout.id);
    assert_eq!(entry["goal_id"], goal.id);
    assert_eq!(entry["goal_title"], "Ship the relay");
    assert_eq!(entry["directive_id"], directive.id);
    assert_eq!(entry["directive_title"], "Cut the release");
    assert_eq!(
        entry["reason"], "external_dependency",
        "the reason must be the snake_case id the UI keys its icon off, not a debug name"
    );
    assert_eq!(entry["note"], "waiting on the relay to come up");
    // `assert_eq!(…, true)` is a clippy lint; what matters here is the
    // value is a JSON bool and not the string "true".
    assert!(
        entry["still_blocked"]
            .as_bool()
            .expect("still_blocked is a bool"),
        "an ExternalDependency bailout leaves the directive blocked forever: {entry}"
    );

    // `bailouts` has no date column: the day comes from the HLC's
    // physical nanoseconds, so it is checked as a real local calendar
    // date near today rather than a formatted substring — a
    // seconds-vs-nanoseconds mix-up yields a valid-looking string
    // thousands of years out, which a length check would pass.
    let date = entry["date"].as_str().expect("date string");
    let parsed = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .unwrap_or_else(|e| panic!("entropy_log date {date:?} is not YYYY-MM-DD: {e}"));
    let drift = (parsed - chrono::Local::now().date_naive())
        .num_days()
        .abs();
    assert!(
        drift <= 1,
        "a bailout recorded just now must land on today's local date, got {date} ({drift} days off)"
    );
}
