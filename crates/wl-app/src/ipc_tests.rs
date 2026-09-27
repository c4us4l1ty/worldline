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

/// `master_plan_preview(provider, model, intent, target_date, complexity)`
/// cannot succeed in a test — there is no BYOK key and no network. It
/// must, however, get far enough to read the vault and report the missing
/// key, which proves the payload bound. The reverse order matters: a key
/// mismatch would surface as "missing required key" long before the vault
/// is touched.
#[test]
fn master_plan_preview_binds_its_payload_before_touching_the_vault() {
    let (app, _scratch) = mock_app();

    let result = call(
        &app,
        "master_plan_preview",
        serde_json::json!({
            "provider": "openrouter",
            "model": "anthropic/claude-sonnet-5",
            "intent": "in n out burger",
            "target_date": serde_json::Value::Null,
            "complexity": 3,
        }),
    );
    assert_bound(&result, "master_plan_preview");
    let err = result.expect_err("no API key is configured in this install");
    assert!(
        err.contains("no API key stored for provider openrouter"),
        "master_plan_preview bound its arguments but did not reach the vault: {err}"
    );
}

/// The blank-model refusal must fire BEFORE the vault and before any
/// network call, and it must name the setting to fix rather than
/// reporting "offline".
///
/// This is the regression for the placeholder the compose screen used to
/// send when nothing was configured: `"flagship"`, which is not a model
/// id on any provider, so every AI request 400'd and the UI blamed the
/// network. (`master_plan_preview` is the only AI command left that takes
/// a model, so it is the only one that can hit this.)
#[test]
fn a_blank_model_is_refused_with_a_pointer_to_settings() {
    let (app, _scratch) = mock_app();

    let result = call(
        &app,
        "master_plan_preview",
        serde_json::json!({
            "provider": "openrouter",
            "model": "",
            "intent": "in n out burger",
            "target_date": serde_json::Value::Null,
            "complexity": 3,
        }),
    );
    assert_bound(&result, "master_plan_preview");
    let err = result.expect_err("a blank model must be refused");
    assert!(
        err.contains("no model selected") && err.contains("Settings"),
        "master_plan_preview must refuse a blank model and point at Settings, got: {err}"
    );
}

/// An out-of-range rating is refused before the billable call, not after
/// the write. It is an `i64` so a key mismatch would fail loudly, but the
/// VALUE has to be checked too: a goal stored at 99/5 would be silently
/// clamped on read and quietly poison the estimator's buckets.
#[test]
fn an_out_of_range_rating_is_refused_before_the_call() {
    let (app, _scratch) = mock_app();
    for bad in [0, 6, -1] {
        let result = call(
            &app,
            "master_plan_preview",
            serde_json::json!({
                "provider": "openrouter",
                "model": "anthropic/claude-sonnet-5",
                "intent": "in n out burger",
                "target_date": serde_json::Value::Null,
                "complexity": bad,
            }),
        );
        let err = result.expect_err("an out-of-range rating must be refused");
        assert!(
            err.contains("complexity"),
            "the refusal must name the field, got: {err}"
        );
    }
}

/// The whole Tier-1 path is now two commands, and the split is the point:
/// a fetch that persists nothing, and a commit that takes the plan the
/// user edited. This drives the second half over the real handler list
/// with a hand-built plan — a `struct` parameter, so the UI has to wrap
/// it, and a payload whose every field is asserted off the persisted rows
/// because two of them (`complexity`, and the edge) are exactly the kind
/// that go missing silently.
#[test]
fn commit_plan_writes_the_edited_plan_and_its_edges() {
    let (app, _scratch) = mock_app();
    let state = app.state::<Arc<AppState>>().inner().clone();

    let preview = serde_json::json!({
        "plan": {
            "title": "Ship the relay",
            "description": serde_json::Value::Null,
            "milestones": [{
                "title": "Wire the protocol",
                "description": serde_json::Value::Null,
                "rationale": "nothing else can be tested without it",
                "directives": [
                    {
                        "title": "Draft the outline",
                        "execution_context": "on paper",
                        "estimated_minutes": 20,
                        "phases": [],
                        "after": serde_json::Value::Null,
                    },
                    {
                        "title": "Write the draft",
                        "execution_context": serde_json::Value::Null,
                        "estimated_minutes": 25,
                        "phases": [],
                        "after": 0,
                    },
                ],
            }],
        },
        "repair": { "notes": [] },
        "complexity": 5,
        "fallback": serde_json::Value::Null,
    });

    let out = call(
        &app,
        "commit_plan",
        serde_json::json!({
            "preview": preview,
            "target_date": "2026-12-31",
            "intent": "ship the relay",
        }),
    )
    .expect("commit_plan must bind and succeed");
    assert_bound(&Ok(out.clone()), "commit_plan");

    let goal_id = out["goal_id"].as_str().expect("goal id").to_string();
    let goal = state.repos.goal(&goal_id).unwrap().expect("goal persisted");
    assert_eq!(goal.title, "Ship the relay");
    assert_eq!(goal.target_date.as_deref(), Some("2026-12-31"));
    // The rating rode inside the preview payload rather than as a second
    // argument, and survived: it is what the estimator buckets on.
    assert_eq!(
        goal.complexity, 5,
        "a rating dropped between fetch and commit poisons the estimator permanently"
    );

    let ms = state.repos.milestones_for_goal(&goal_id).unwrap();
    assert_eq!(ms.len(), 1);
    let runnable = state.repos.runnable_directives(&today_local()).unwrap();
    let titles: Vec<&str> = runnable.iter().map(|d| d.title.as_str()).collect();
    assert_eq!(
        titles,
        vec!["Draft the outline"],
        "only the task with nothing blocking it may be offered"
    );
    // The edge itself, read off the row rather than off the write path.
    let ledger = state.repos.task_ledger(10).unwrap();
    let second = ledger
        .iter()
        .find(|r| r.title == "Write the draft")
        .expect("the second task is on the ledger");
    assert_eq!(
        second.blocked_by.as_deref(),
        Some("Draft the outline"),
        "an edge that did not survive the write is an unreadable graph"
    );
    // …and clearing it makes the second task runnable, which is the whole
    // point of an edge the user can edit.
    state
        .repos
        .set_directive_state(
            &runnable[0].id,
            wl_core::domain::DirectiveState::Completed,
            None,
        )
        .unwrap();
    let after = state.repos.runnable_directives(&today_local()).unwrap();
    assert!(
        after.iter().any(|d| d.title == "Write the draft"),
        "finishing the prerequisite must unlock the task it blocked"
    );
    assert!(
        !after.iter().any(|d| d.title == "Draft the outline"),
        "and a finished task must leave the runnable pool"
    );
}

/// The ledger's own write, over the real handler list: a tick resolves a
/// task, and the response is the canvas the user lands on.
///
/// The assertion is on the RESULT rather than on "it did not error",
/// because the failure this guards is a tick that reports success and
/// leaves the canvas showing a task that is already done.
#[test]
fn marking_a_task_done_advances_the_canvas() {
    let (app, _scratch) = mock_app();
    let state = app.state::<Arc<AppState>>().inner().clone();
    let created = call(
        &app,
        "create_goal",
        serde_json::json!({
            "title": "Two tasks",
            "description": serde_json::Value::Null,
            "target_date": serde_json::Value::Null,
            "complexity": 3,
        }),
    )
    .expect("create_goal");
    let _ = created;
    let ids: Vec<String> = state
        .repos
        .runnable_directives(&today_local())
        .unwrap()
        .into_iter()
        .map(|d| d.id)
        .collect();
    assert_eq!(ids.len(), 1, "create_goal seeds one task");

    let ledger = call(&app, "task_ledger", serde_json::json!({})).expect("task_ledger");
    let rows = ledger.as_array().expect("array");
    assert_eq!(rows.len(), 1);
    // Field NAMES are the UI's destructuring contract, and a renamed field
    // drops to `undefined` there rather than failing here.
    let mut keys: Vec<&str> = rows[0]
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "blocked_by",
            "complexity_label",
            "directive_id",
            "estimated_minutes",
            "goal_id",
            "goal_title",
            "instruction",
            "milestone_title",
            "phase",
            "state",
            "title",
        ],
        "task_ledger field names are the UI contract: {:?}"
        ,
        rows
    );
    assert_eq!(rows[0]["complexity_label"], "standard");

    let ticked = call(
        &app,
        "mark_task_done",
        serde_json::json!({ "directive_id": ids[0] }),
    )
    .expect("mark_task_done");
    assert_bound(&Ok(ticked.clone()), "mark_task_done");
    // The seeded task is the only one, so the canvas goes idle — which is
    // the observable proof that the tick advanced the canvas rather than
    // only flipping a row.
    assert_eq!(ticked["state"], "idle", "the canvas must advance: {ticked}");

    // A second tick on the same row is a checkbox being confirmed, not an
    // error the UI would have to render as a failure.
    call(
        &app,
        "mark_task_done",
        serde_json::json!({ "directive_id": ids[0] }),
    )
    .expect("a second tick is idempotent");

    // A task that does not exist names itself, rather than reporting a
    // generic failure.
    let missing = call(
        &app,
        "mark_task_done",
        serde_json::json!({ "directive_id": "dir-nope" }),
    )
    .expect_err("no such task");
    assert!(
        missing.contains("dir-nope"),
        "the error must name the id the caller sent: {missing}"
    );
}

/// The estimator, over the real handler list, with the rating a real flow
/// would have written.
#[test]
fn the_calibration_view_reports_the_record_with_its_sample_size() {
    let (app, _scratch) = mock_app();
    let state = app.state::<Arc<AppState>>().inner().clone();
    call(
        &app,
        "create_goal",
        serde_json::json!({
            "title": "Heavy work",
            "description": serde_json::Value::Null,
            "target_date": serde_json::Value::Null,
            "complexity": 4,
        }),
    )
    .expect("create_goal");
    let ids: Vec<String> = state
        .repos
        .runnable_directives(&today_local())
        .unwrap()
        .into_iter()
        .map(|d| d.id)
        .collect();
    state
        .repos
        .set_directive_state(&ids[0], wl_core::domain::DirectiveState::Completed, None)
        .unwrap();

    let view = call(&app, "calibration_view", serde_json::json!({})).expect("calibration_view");
    let rows = view.as_array().expect("array");
    assert_eq!(rows.len(), 1, "one bucket: the one that was rated");
    assert_eq!(rows[0]["complexity"], 4);
    assert_eq!(rows[0]["label"], "heavy");
    // The counts travel with the percentage. A bare 63% is the "inert
    // number" failure this project does not ship.
    assert_eq!(rows[0]["evidence"], "1 of 1");
    assert_eq!(rows[0]["observations"], 1);
    let pct = rows[0]["percent"].as_i64().expect("an integer percent");
    assert!(
        (60..=70).contains(&pct),
        "one success on a 0.625 prior should stay near it, got {pct}"
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

/// The control panel's goal rail, over the real handler list.
///
/// It takes no arguments, so there is no payload to mis-bind here — what
/// can be wrong is the row that crosses back, and it is wrong quietly.
/// Hence the field NAMES are asserted, not just the values: the UI
/// destructures by name, and a renamed field drops to `undefined` there.
#[test]
fn the_goal_rail_reports_progress() {
    let (app, _scratch) = mock_app();
    let state = app.state::<Arc<AppState>>().inner().clone();
    let repos = &state.repos;

    // Seeded through `Repos` rather than `create_goal`: this test is
    // about the read path, and `create_goal`'s starter milestone would make
    // `milestone_total` 1 forever.
    let goal = repos
        .create_goal("Ship the relay", None, Some("2026-12-31"), 4, None)
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

    // One of the two milestones is done, so the rail's fraction is a real
    // 1-of-2 rather than 0-of-2 or 1-of-1. `assert_bound` is vacuous for a
    // zero-argument command today, but it is the canary for the day one of
    // these grows a parameter: a `target_date`-style key that stops binding
    // is silent here too.
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

    // …and the ledger carries the same goal's tasks, joined up to it.
    repos
        .create_directive(
            &second.id,
            "Cut the release",
            None,
            25,
            1,
            &today_local(),
            None,
            &[],
            None,
        )
        .expect("directive");
    let _ = first;
    let _ = second;
    let ledger = call(&app, "task_ledger", serde_json::json!({})).expect("task_ledger");
    let rows = ledger.as_array().expect("array");
    assert_eq!(rows.len(), 1, "one task was seeded: {ledger}");
    let row = &rows[0];
    assert_eq!(row["title"], "Cut the release");
    assert_eq!(row["goal_title"], "Ship the relay");
    assert_eq!(row["milestone_title"], "Ship the binary");
    // The rating, resolved to a WORD in the core so the UI cannot keep a
    // second table of them and drift.
    assert_eq!(row["complexity_label"], "heavy");
    // Open work, so nothing is blocking it.
    assert_eq!(row["state"], "queued");
    assert_eq!(row["blocked_by"], serde_json::Value::Null);
    // Monolithic, so no phase badge — a null rather than a fake (1, 1).
    assert_eq!(row["phase"], serde_json::Value::Null);

}
