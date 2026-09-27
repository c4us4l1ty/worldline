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
        relay_url: std::sync::Mutex::new(None),
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

/// The manual authoring path, end to end through IPC: the compose
/// screen's `create_goal`, then the two authoring commands behind it.
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
        }),
    )
    .expect("create_goal must bind and succeed");
    assert_bound(&Ok(created.clone()), "create_goal");
    let goal_id = created["id"].as_str().expect("goal id").to_string();
    assert_eq!(created["title"], "Ship the relay");
    // The horizon picker resolves to a real calendar date before it
    // crosses the wire, so the store's shape-exact date check must
    // accept what the UI produced.
    assert_eq!(created["target_date"], "2026-12-31");

    // `create_manual_milestone(goal_id, title, description)`.
    let milestone = call(
        &app,
        "create_manual_milestone",
        serde_json::json!({
            "goal_id": goal_id,
            "title": "Wire format",
            "description": serde_json::Value::Null,
        }),
    )
    .expect("create_manual_milestone must bind and succeed");
    assert_bound(&Ok(milestone.clone()), "create_manual_milestone");
    let milestone_id = milestone.as_str().expect("milestone id").to_string();

    // `create_manual_directive(milestone_id, title, execution_context,
    // estimated_minutes, scheduled_for_date)` — four multi-word keys.
    let scheduled = chrono::Local::now().format("%Y-%m-%d").to_string();
    let directive = call(
        &app,
        "create_manual_directive",
        serde_json::json!({
            "milestone_id": milestone_id,
            "title": "Write the push validator",
            "execution_context": "crates/wl-protocol/src/lib.rs",
            "estimated_minutes": 25,
            "scheduled_for_date": scheduled,
        }),
    )
    .expect("create_manual_directive must bind and succeed");
    let directive_id = directive.as_str().expect("directive id").to_string();

    // The manual directive must be persisted with the exact field values
    // the payload carried. `execution_context` and `estimated_minutes`
    // are the two a key mismatch would quietly lose: both are
    // `Option`/integer at the store boundary, so a row with them
    // defaulted still satisfies every constraint the store checks.
    //
    // Not asserted through `current_directive`: `create_goal` seeds a
    // runnable directive titled from the goal (B-002), and that one
    // legitimately owns the canvas for today. Read the row back instead.
    let state = app.state::<Arc<AppState>>().inner().clone();
    let stored = state
        .repos
        .directive(&directive_id)
        .expect("directive lookup")
        .expect("manual directive was persisted");
    assert_eq!(stored.title, "Write the push validator");
    assert_eq!(
        stored.execution_context.as_deref(),
        Some("crates/wl-protocol/src/lib.rs")
    );
    assert_eq!(stored.estimated_minutes, 25);
    assert_eq!(stored.scheduled_for_date, scheduled);
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
            "model": "flagship",
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
